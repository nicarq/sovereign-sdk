use sov_attester_incentives::AttesterIncentives;
use sov_bank::IntoPayable;
use sov_modules_api::macros::config_value;
use sov_modules_api::transaction::{PriorityFeeBips, SequencerReward, UnsignedTransaction};
use sov_modules_api::{EncodeCall, Gas, ModuleInfo};
use sov_modules_stf_blueprint::TxEffect;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{
    assert_matches, generate_optimistic_runtime, AsUser, BatchTestCase, TestUser, TransactionType,
    TEST_DEFAULT_USER_BALANCE,
};
use sov_value_setter::{CallMessage, ValueSetter};

use crate::stf_blueprint::{
    get_balance, get_seq_bond, setup, TransactionTestCase, TxProcessingError,
};
type S = sov_test_utils::TestSpec;

generate_optimistic_runtime!(IntegTestRuntime <= value_setter: ValueSetter<S>);

// Check if `chain_id` is validated in the stf blueprint.
#[test]
fn test_enforces_chain_id() {
    let mut genesis_config = HighLevelOptimisticGenesisConfig::generate();
    genesis_config
        .additional_accounts
        .push(TestUser::<S>::generate(TEST_DEFAULT_USER_BALANCE));

    let admin_account = genesis_config.additional_accounts[0].clone();

    let genesis = GenesisConfig::from_minimal_config(
        genesis_config.clone().into(),
        sov_value_setter::ValueSetterConfig {
            admin: admin_account.address(),
        },
    );

    let mut runner: TestRunner<IntegTestRuntime<S>, S> =
        TestRunner::new_with_genesis(genesis.into_genesis_params(), Default::default());
    let encoded_message =
        <IntegTestRuntime<S> as EncodeCall<ValueSetter<S>>>::encode_call(CallMessage::SetValue(8));

    let real_chain_id = config_value!("CHAIN_ID");

    let utx = UnsignedTransaction::new(
        encoded_message.clone(),
        real_chain_id,
        100.into(),
        100_000_000,
        0,
        None,
    );
    let tx = TransactionType::<ValueSetter<S>, S>::pre_signed(utx, admin_account.private_key());

    runner.execute_transaction(TransactionTestCase {
        input: tx,
        assert: Box::new(move |result, _state| assert!(result.tx_receipt.is_successful())),
    });

    let fake_chain_id = real_chain_id + 1;
    let invalid_utx = UnsignedTransaction::new(
        encoded_message,
        fake_chain_id,
        100.into(),
        100_000_000,
        0,
        None,
    );

    runner.execute_batch(BatchTestCase {
        input: vec![TransactionType::<ValueSetter<S>, S>::pre_signed(
            invalid_utx,
            admin_account.private_key(),
        )]
        .into(),
        assert: Box::new(move |result, _state| {
            let batch_receipt = result.batch_receipt.as_ref().unwrap();
            let tx_receipts = &batch_receipt.tx_receipts;

            assert_eq!(tx_receipts.len(), 1);

            match &tx_receipts[0].receipt {
                sov_modules_api::TxEffect::Skipped(skipped) => {
                    assert_matches!(skipped.error, TxProcessingError::AuthenticationFailed(_));
                }
                unexpected => panic!("Expected TxEffect::Skipped but got {:?}", unexpected),
            }

            assert_eq!(
                batch_receipt.inner.outcome,
                sov_modules_api::BatchSequencerOutcome::Rewarded(SequencerReward(0))
            );
        }),
    });
}

// Execute batch of valid transactions and ensure that the relevant (sequencer, attester, user) balances ware updater correctly/
#[test]
fn execute_many_successful_tx_test() {
    let (mut runner, admin_account, sequencer_account) = setup();
    let sequencer_da_address = sequencer_account.da_address;

    let attester_module = AttesterIncentives::<S>::default();

    let priority_fee_bips = PriorityFeeBips::from_percentage(5);

    let start_admin_balance =
        runner.query_state(|state| get_balance(&admin_account.address(), state));

    let start_attester_module_balance =
        runner.query_state(|state| get_balance(attester_module.id().to_payable(), state));

    let start_sequencer_bond =
        runner.query_state(|state| get_seq_bond(&sequencer_da_address, state));

    let nb_tx = 8;
    let mut txs = Vec::default();

    for _ in 0..nb_tx {
        let tx = admin_account
            .create_plain_message::<ValueSetter<S>>(sov_value_setter::CallMessage::SetValue(10))
            .with_max_fee(200_000)
            .with_max_priority_fee_bips(priority_fee_bips);

        txs.push(tx);
    }

    runner.execute_batch(BatchTestCase {
        input: txs.into(),
        assert: Box::new(move |result, state| {
            let batch_receipt = result.batch_receipt.as_ref().unwrap();

            let gas_price = &batch_receipt.gas_price;
            let tx_receipts = &batch_receipt.tx_receipts;

            assert_eq!(tx_receipts.len(), nb_tx);

            let mut seq_fee = 0;
            let mut total_gas_value = 0;

            for tx_receipt in tx_receipts {
                match &tx_receipt.receipt {
                    TxEffect::Successful(tx_contents) => {
                        let gas_value = tx_contents.gas_used.value(gas_price);
                        total_gas_value += gas_value;
                        seq_fee += priority_fee_bips.apply(gas_value).unwrap();
                    }
                    unexpected => panic!("Expected TxEffect::Successful but got {:?}", unexpected),
                }
            }

            let end_admin_balance = get_balance(&admin_account.address(), state);
            let end_attester_module_balance = get_balance(attester_module.id().to_payable(), state);
            let end_sequencer_bond = get_seq_bond(&sequencer_da_address, state);

            assert_eq!(
                end_admin_balance,
                start_admin_balance - seq_fee - total_gas_value
            );

            assert_eq!(end_sequencer_bond, start_sequencer_bond + seq_fee);

            assert_eq!(
                end_attester_module_balance,
                start_attester_module_balance + total_gas_value
            );

            assert_eq!(
                batch_receipt.inner.outcome,
                sov_modules_api::BatchSequencerOutcome::Rewarded(SequencerReward(seq_fee))
            );
        }),
    });
}
