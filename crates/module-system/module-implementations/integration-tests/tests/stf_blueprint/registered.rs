use std::env;

use serial_test::serial;
use sov_modules_api::transaction::{PriorityFeeBips, SequencerReward};
use sov_modules_api::{BatchSequencerOutcome, Gas, GasSpec};
use sov_modules_stf_blueprint::TxEffect;
use sov_test_utils::BatchTestCase;

use super::{create_txs, TxStatus};
use crate::stf_blueprint::setup;

type S = sov_test_utils::TestSpec;

fn check_txs(tx_statuses: Vec<TxStatus>, priority_fee_bips: PriorityFeeBips) {
    let (mut runner, actors) = setup();

    let txs = create_txs(
        &tx_statuses,
        priority_fee_bips,
        &actors.admin_account,
        &actors.not_admin_account,
    );

    let start = runner.query_state(|state| actors.balances(state));

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
                    TxEffect::Reverted(tx_contents) => {
                        // From gas usage point of view the `Successful & Reverted` cases are the same.
                        let gas_value = tx_contents.gas_used.value(gas_price);
                        gas_value_charged_to_user += gas_value;
                        seq_fee += priority_fee_bips.apply(gas_value).unwrap();
                        valid_tx_count += 1;
                    }
                }
            }

            assert_eq!(nb_of_valid_txs, valid_tx_count);
            assert_eq!(nb_of_skipped_txs, skipped_tx_count);

            let end = actors.balances(state);

            // Check user balances.
            assert_eq!(
                end.admin_balance + end.not_admin_balance,
                start.admin_balance + start.not_admin_balance - seq_fee - gas_value_charged_to_user
            );

            // Check sequencer rewards.
            assert_eq!(
                end.sequencer_bond,
                start.sequencer_bond + seq_fee - batch_hook_gas_value - seq_penalty
            );

            // Check prover rewards.
            assert_eq!(
                end.attester_module_balance,
                start.attester_module_balance
                    + gas_value_charged_to_user
                    + batch_hook_gas_value
                    + seq_penalty
            );

            // This has already been tested by previous assertions, but here we explicitly clarify that no money is created or lost.
            assert_eq!(end.total_balance(), start.total_balance());

            assert_eq!(
                batch_receipt.inner.outcome,
                // TODO account for batch_hook_gas_value
                sov_modules_api::BatchSequencerOutcome::Rewarded(SequencerReward(seq_fee))
            );
        }),
    });
}

// Execute batch of valid transactions and ensure that the relevant balances ware updated correctly
#[test]
// All the tests run serially because they modify the shared env variables.
#[serial]
fn execute_many_successful_tx_test() {
    env::set_var("SOV_SDK_CONST_OVERRIDE_BATCH_HOOK_GAS", "[10, 10]");
    let priority_fee_bips = PriorityFeeBips::from_percentage(5);
    let tx_statuses = vec![
        TxStatus::Success,
        TxStatus::Success,
        TxStatus::Success,
        TxStatus::Success,
        TxStatus::Success,
    ];
    check_txs(tx_statuses, priority_fee_bips);
}

// Execute a batch of mixed transactions and ensure that the relevant balances were updated correctly
#[test]
#[serial]
fn execute_batch_of_valid_and_invalid_tx_test() {
    env::set_var("SOV_SDK_CONST_OVERRIDE_BATCH_HOOK_GAS", "[10, 10]");
    let priority_fee_bips = PriorityFeeBips::from_percentage(5);
    let tx_statuses = vec![
        TxStatus::Success,
        TxStatus::BadSignature,
        TxStatus::Success,
        TxStatus::BadChainId,
        TxStatus::BadNonce,
        TxStatus::Success,
        TxStatus::Reverted,
    ];
    check_txs(tx_statuses, priority_fee_bips);
}

// Execute a batch of invalid transactions and ensure that the relevant balances ware updated correctly
#[test]
#[serial]
fn execute_batch_of_invalid_tx_test() {
    env::set_var("SOV_SDK_CONST_OVERRIDE_BATCH_HOOK_GAS", "[10, 10]");
    let priority_fee_bips = PriorityFeeBips::from_percentage(5);
    let tx_statuses = vec![
        TxStatus::BadChainId,
        TxStatus::BadNonce,
        TxStatus::BadNonce,
        TxStatus::BadChainId,
        TxStatus::BadSignature,
    ];
    check_txs(tx_statuses, priority_fee_bips);
}

// If the sequencer can't pay for the batch hooks execution, we exit early without processing the transactions.
#[test]
#[serial]
fn not_enough_stake_to_execute_batch_hook_test() {
    env::set_var(
        "SOV_SDK_CONST_OVERRIDE_BATCH_HOOK_GAS",
        "[2000000, 2000000]",
    );

    let priority_fee_bips = PriorityFeeBips::from_percentage(5);
    let tx_statuses = vec![
        TxStatus::Success,
        TxStatus::Success,
        TxStatus::Success,
        TxStatus::Success,
        TxStatus::Success,
    ];

    let (mut runner, actors) = setup();

    let txs = create_txs(
        &tx_statuses,
        priority_fee_bips,
        &actors.admin_account,
        &actors.not_admin_account,
    );

    let batch_hook_gas = <S as GasSpec>::batch_hook_gas();
    let seq_address = actors.sequencer_account.da_address;

    let start = runner.query_state(|state| actors.balances(state));

    runner.execute_batch(BatchTestCase {
        input: txs.into(),
        assert: Box::new(move |result,state| {
            let batch_receipt = result.batch_receipt.as_ref().unwrap();
            assert!(batch_receipt.tx_receipts.is_empty());

            let gas_price = &batch_receipt.gas_price;
            let batch_hook_gas_value = batch_hook_gas.value(gas_price);

            let err_str = format!("Not enough gas to execute `begin_batch_hook`: Sequencer's: {} stake is too low. Current stake: {}, amount to deduct: {}", seq_address, start.sequencer_bond, batch_hook_gas_value);
            assert_eq!(
                batch_receipt.inner.outcome,
                BatchSequencerOutcome::Ignored(err_str)
            );

            let end = actors.balances(state);

            // Balances didn't change.
            assert_eq!(end, start);
        }),
    });
}

// If the sequencer can't pay for the batch authentication, we exit early without processing the transactions.
#[test]
#[serial]
fn not_enough_stake_auth_batch_test() {
    env::set_var("SOV_SDK_CONST_OVERRIDE_BATCH_HOOK_GAS", "[900000, 900000]");

    let priority_fee_bips = PriorityFeeBips::from_percentage(5);
    let tx_statuses = vec![
        TxStatus::Success,
        TxStatus::Success,
        TxStatus::Success,
        TxStatus::Success,
        TxStatus::Success,
    ];

    let (mut runner, actors) = setup();

    let txs = create_txs(
        &tx_statuses,
        priority_fee_bips,
        &actors.admin_account,
        &actors.not_admin_account,
    );

    let max_tx_check_costs = <S as GasSpec>::max_tx_check_costs();
    let batch_hook_gas = <S as GasSpec>::batch_hook_gas();

    let start = runner.query_state(|state| actors.balances(state));

    runner.execute_batch(BatchTestCase {
        input: txs.into(),
        assert: Box::new(move |result, state| {
            let batch_receipt = result.batch_receipt.as_ref().unwrap();
            assert!(batch_receipt.tx_receipts.is_empty());

            let gas_price = &batch_receipt.gas_price;
            // Sequencer paid for the batch hook execution.
            let batch_hook_gas_value = batch_hook_gas.value(gas_price);
            let stake_left = start.sequencer_bond - batch_hook_gas_value;
            let batch_auth_gas_value = max_tx_check_costs.value(gas_price)*(tx_statuses.len() as u64);
            // TODO add sequencer address to the error str.
            let err_str = format!("Not enough gas to authenticate the batch: The amount staked by the sequencer is less than the minimum bond. Amount currently staked: {}, minimum bond amount: {}.", stake_left, batch_auth_gas_value);
            assert_eq!(
                batch_receipt.inner.outcome,
                BatchSequencerOutcome::Ignored(err_str)
            );

            let end = actors.balances(state);
            assert_eq!(end.sequencer_bond, start.sequencer_bond - batch_hook_gas_value);
            assert_eq!(end.attester_module_balance, start.attester_module_balance + batch_hook_gas_value);

            assert_eq!(end.admin_balance, start.admin_balance);
            assert_eq!(end.not_admin_balance, start.not_admin_balance);

            assert_eq!(end.total_balance(), start.total_balance());
        }),
    });
}
