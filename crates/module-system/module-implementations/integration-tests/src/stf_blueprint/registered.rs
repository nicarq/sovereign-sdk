use sov_attester_incentives::AttesterIncentives;
use sov_bank::IntoPayable;
use sov_modules_api::transaction::{PriorityFeeBips, SequencerReward};
use sov_modules_api::{Gas, GasSpec, ModuleInfo};
use sov_modules_stf_blueprint::TxEffect;
use sov_test_utils::BatchTestCase;

use super::{create_txs, TxStatus};
use crate::stf_blueprint::{get_balance, get_seq_bond, setup};

type S = sov_test_utils::TestSpec;

fn check_txs(tx_statuses: Vec<TxStatus>, priority_fee_bips: PriorityFeeBips) {
    let (mut runner, admin_account, sequencer_account) = setup();

    let txs = create_txs(&tx_statuses, priority_fee_bips, &admin_account);

    let sequencer_da_address = sequencer_account.da_address;

    let attester_module = AttesterIncentives::<S>::default();

    let start_admin_balance =
        runner.query_state(|state| get_balance(&admin_account.address(), state));

    let start_attester_module_balance =
        runner.query_state(|state| get_balance(attester_module.id().to_payable(), state));

    let start_sequencer_bond =
        runner.query_state(|state| get_seq_bond(&sequencer_da_address, state));

    let txs_len = txs.len();
    let nb_of_valid_txs = TxStatus::nb_of_valid_txs(&tx_statuses);
    let nb_of_skipped_txs = TxStatus::nb_of_skipped_txs(&tx_statuses);

    runner.execute_batch(BatchTestCase {
        input: txs.into(),
        assert: Box::new(move |result, state| {
            let batch_receipt = result.batch_receipt.as_ref().unwrap();

            let gas_price = &batch_receipt.gas_price;
            let tx_receipts = &batch_receipt.tx_receipts;

            assert_eq!(tx_receipts.len(), txs_len);

            let mut seq_fee = 0;
            let mut seq_penalty = 0;
            let mut gas_value_charged_to_user = 0;

            let batch_hook_gas_value = <S as GasSpec>::batch_hook_gas().value(gas_price);

            let mut valid_tx_count = 0;
            let mut skipped_tx_count = 0;
            for tx_receipt in tx_receipts {
                match &tx_receipt.receipt {
                    TxEffect::Successful(tx_contents) => {
                        let gas_value = tx_contents.gas_used.value(gas_price);
                        gas_value_charged_to_user += gas_value;
                        seq_fee += priority_fee_bips.apply(gas_value).unwrap();
                        valid_tx_count += 1;
                    }
                    TxEffect::Skipped(tx_contents) => {
                        let gas_value = tx_contents.gas_used.value(gas_price);
                        // Sequencer doesn't get the fee and is penalized
                        seq_penalty += gas_value;
                        skipped_tx_count += 1;
                    }
                    TxEffect::Reverted(_) => {
                        todo!()
                    }
                }
            }

            assert_eq!(nb_of_valid_txs, valid_tx_count);
            assert_eq!(nb_of_skipped_txs, skipped_tx_count);

            let end_admin_balance = get_balance(&admin_account.address(), state);
            let end_attester_module_balance = get_balance(attester_module.id().to_payable(), state);
            let end_sequencer_bond = get_seq_bond(&sequencer_da_address, state);

            assert_eq!(
                end_admin_balance,
                start_admin_balance - seq_fee - gas_value_charged_to_user
            );

            assert_eq!(
                end_sequencer_bond,
                start_sequencer_bond + seq_fee - batch_hook_gas_value - seq_penalty
            );

            assert_eq!(
                end_attester_module_balance,
                start_attester_module_balance
                    + gas_value_charged_to_user
                    + batch_hook_gas_value
                    + seq_penalty
            );

            assert_eq!(
                batch_receipt.inner.outcome,
                // TODO account for batch_hook_gas_value
                sov_modules_api::BatchSequencerOutcome::Rewarded(SequencerReward(seq_fee))
            );
        }),
    });
}

// Execute batch of valid transactions and ensure that the relevant (sequencer, attester, user) balances ware updated correctly
#[test]
fn execute_many_successful_tx_test() {
    //env::set_var("SOV_SDK_CONST_OVERRIDE_BATCH_HOOK_GAS", "[10, 10]");
    let priority_fee_bips = PriorityFeeBips::from_percentage(5);
    let tx_statuses = vec![TxStatus::Valid, TxStatus::Valid, TxStatus::Valid];
    check_txs(tx_statuses, priority_fee_bips);
}

// Execute batch of mixes transactions and ensure that the relevant (sequencer, attester, user) balances ware updated correctly
#[test]
fn execute_batch_of_valid_and_invalid_tx_test() {
    let priority_fee_bips = PriorityFeeBips::from_percentage(5);
    let tx_statuses = vec![
        TxStatus::Valid,
        TxStatus::Valid,
        TxStatus::BadChainId,
        TxStatus::BadNonce,
        TxStatus::Valid,
    ];
    check_txs(tx_statuses, priority_fee_bips);
}
