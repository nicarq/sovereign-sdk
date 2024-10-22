mod registered;

use sov_bank::Bank;
use sov_modules_api::macros::config_value;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::transaction::{PriorityFeeBips, UnsignedTransaction};
use sov_modules_api::{ApiStateAccessor, DaSpec, EncodeCall, Spec};
use sov_sequencer_registry::SequencerRegistry;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::{config_gas_token_id, Payable, TestRunner};
use sov_test_utils::{
    generate_optimistic_runtime, TestSequencer, TestUser, TransactionType,
    TEST_DEFAULT_USER_BALANCE,
};
use sov_value_setter::{CallMessage, ValueSetter};

type S = sov_test_utils::TestSpec;

generate_optimistic_runtime!(IntegTestRuntime <= value_setter: ValueSetter<S>);

pub(crate) fn get_balance(payable: impl Payable<S>, state: &mut ApiStateAccessor<S>) -> u64 {
    Bank::<S>::default()
        .get_balance_of(payable, config_gas_token_id(), state)
        .unwrap_infallible()
        .unwrap()
}

pub(crate) fn get_seq_bond(
    sequencer_da_address: &<<S as Spec>::Da as DaSpec>::Address,
    state: &mut ApiStateAccessor<S>,
) -> u64 {
    let sequencer_module = SequencerRegistry::<S>::default();
    sequencer_module
        .is_sender_allowed(sequencer_da_address, state)
        .unwrap()
        .balance
}

pub(crate) fn setup() -> (
    TestRunner<IntegTestRuntime<S>, S>,
    TestUser<S>,
    TestSequencer<S>,
) {
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

    let runner: TestRunner<IntegTestRuntime<S>, S> =
        TestRunner::new_with_genesis(genesis.into_genesis_params(), Default::default());

    let admin_account = genesis_config.additional_accounts[0].clone();
    let sequencer_account = genesis_config.initial_sequencer.clone();

    (runner, admin_account, sequencer_account)
}

fn create_tx_bad_chain_id(
    nonce: u64,
    max_priority_fee_bips: PriorityFeeBips,
    signer: &TestUser<S>,
) -> TransactionType<ValueSetter<S>, S> {
    let encoded_message =
        <IntegTestRuntime<S> as EncodeCall<ValueSetter<S>>>::encode_call(CallMessage::SetValue(8));

    let utx = UnsignedTransaction::new(
        encoded_message.clone(),
        config_value!("CHAIN_ID") + 1,
        max_priority_fee_bips,
        200_000,
        nonce,
        None,
    );

    TransactionType::<ValueSetter<S>, S>::pre_signed(utx, signer.private_key())
}

fn create_tx_valid(
    nonce: u64,
    max_priority_fee_bips: PriorityFeeBips,
    signer: &TestUser<S>,
) -> TransactionType<ValueSetter<S>, S> {
    let encoded_message =
        <IntegTestRuntime<S> as EncodeCall<ValueSetter<S>>>::encode_call(CallMessage::SetValue(8));

    let utx = UnsignedTransaction::new(
        encoded_message.clone(),
        config_value!("CHAIN_ID"),
        max_priority_fee_bips,
        200_000,
        nonce,
        None,
    );

    TransactionType::<ValueSetter<S>, S>::pre_signed(utx, signer.private_key())
}

#[derive(PartialEq, Eq)]
enum TxStatus {
    Valid,
    BadNonce,
    BadChainId,
    // TODO add more conditions
}

impl TxStatus {
    fn nb_of_valid_txs(statuses: &[TxStatus]) -> usize {
        statuses
            .iter()
            .filter(|s| **s == TxStatus::Valid)
            .collect::<Vec<_>>()
            .len()
    }

    fn nb_of_skipped_txs(statuses: &[TxStatus]) -> usize {
        statuses
            .iter()
            .filter(|s| **s != TxStatus::Valid)
            .collect::<Vec<_>>()
            .len()
    }
}

fn create_txs(
    statuses: &[TxStatus],
    max_priority_fee_bips: PriorityFeeBips,
    admin: &TestUser<S>,
) -> Vec<TransactionType<ValueSetter<S>, S>> {
    let mut nonce = 0;
    let mut txs = Vec::new();
    for status in statuses {
        match status {
            TxStatus::Valid => {
                let tx = create_tx_valid(nonce, max_priority_fee_bips, admin);
                txs.push(tx);
                nonce += 1;
            }
            TxStatus::BadNonce => {
                let tx = create_tx_valid(9999, max_priority_fee_bips, admin);
                txs.push(tx);
            }
            TxStatus::BadChainId => {
                let tx = create_tx_bad_chain_id(nonce, max_priority_fee_bips, admin);
                txs.push(tx);
            }
        }
    }
    txs
}
