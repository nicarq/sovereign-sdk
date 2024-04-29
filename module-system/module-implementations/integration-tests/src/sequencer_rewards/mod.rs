use sov_bank::{IntoPayable, GAS_TOKEN_ID};
use sov_mock_da::MockDaSpec;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::transaction::PriorityFeeBips;
use sov_modules_api::{Gas, GasArray, ModuleInfo, Spec};
use sov_modules_stf_blueprint::SequencerOutcome;
use sov_test_utils::runtime::TestRuntime;
use sov_test_utils::value_setter_data::ValueSetterMessages;
use sov_test_utils::{new_test_blob_from_batch, MessageGenerator};

use crate::helpers::{
    AttesterIncentivesParams, BankParams, SequencerParams, TestRollup, DEFAULT_STAKE_AMOUNT,
    GAS_TX_FIXED_COST, S,
};

const TEST_PRIORITY_FEE: PriorityFeeBips = PriorityFeeBips::from_percentage(10);
const INITIAL_USER_BALANCE: u64 = 10_000;
const NUM_TXS_PER_BATCH: u64 = 2;

fn check_sequencer_and_registry_balances(
    rollup: &mut TestRollup,
    seq_rollup_addr: &<S as Spec>::Address,
    expected_sequencer_balance: u64,
    expected_registry_balance: u64,
) {
    let mut checkpoint = rollup.new_state_checkpoint();

    let initial_sequencer_balance = rollup
        .bank()
        .get_balance_of(seq_rollup_addr, GAS_TOKEN_ID, &mut checkpoint)
        .unwrap();

    assert_eq!(initial_sequencer_balance, expected_sequencer_balance);

    // Assert the initial balance of the sequencer registry is equal to the stake amount of the sequencer.
    let seq_registry_balance = rollup
        .bank()
        .get_balance_of(
            rollup.sequencer_registry().id().to_payable(),
            GAS_TOKEN_ID,
            &mut checkpoint,
        )
        .unwrap();

    assert_eq!(seq_registry_balance, expected_registry_balance);
}

fn test_sequencer_reward_in_stf(rollup: &mut TestRollup, max_fee: u64, expected_reward: u64) {
    let value_setter_messages = ValueSetterMessages::prepopulated();
    let value_setter = value_setter_messages.create_raw_txs::<TestRuntime<S, MockDaSpec>>(
        0,
        TEST_PRIORITY_FEE,
        max_fee,
        None,
    );

    assert_eq!(value_setter.len() as u64, NUM_TXS_PER_BATCH);

    let admin_pub_key = value_setter_messages.messages[0].admin.to_address();

    let seq_params = SequencerParams::default();
    let seq_rollup_addr = seq_params.rollup_address;
    let seq_da_addr = seq_params.da_address;
    let bank_params = BankParams::with_addresses_and_balances(vec![
        (seq_params.rollup_address, INITIAL_USER_BALANCE),
        (admin_pub_key, INITIAL_USER_BALANCE),
    ]);
    let attester_params = AttesterIncentivesParams::default();

    // Genesis
    let init_root_hash = rollup.genesis(admin_pub_key, seq_params, bank_params, attester_params);

    let post_genesis_sequencer_balance = INITIAL_USER_BALANCE - DEFAULT_STAKE_AMOUNT;
    let post_genesis_registry_balance = DEFAULT_STAKE_AMOUNT;

    check_sequencer_and_registry_balances(
        rollup,
        &seq_rollup_addr,
        post_genesis_sequencer_balance,
        post_genesis_registry_balance,
    );

    let blob = new_test_blob_from_batch(
        BatchWithId {
            txs: value_setter,
            id: [0; 32],
        },
        seq_da_addr.as_ref(),
        [0; 32],
    );

    let exec_simulation =
        rollup.execution_simulation(1, init_root_hash, vec![blob.clone()], 0, None);

    assert_eq!(exec_simulation.len(), 1, "The execution simulation failed");
    assert_eq!(
        exec_simulation[0].batch_receipts.len(),
        1,
        "The batch execution failed"
    );

    let batch_receipt = &exec_simulation[0].batch_receipts[0];
    assert_eq!(
        batch_receipt.inner,
        SequencerOutcome::Rewarded(expected_reward),
        "The sequencer should get rewarded"
    );

    // The sequencer registry balance should still be equal to the stake amount of the sequencer.
    // The sequencer balance should increase by the expected reward.
    check_sequencer_and_registry_balances(
        rollup,
        &seq_rollup_addr,
        post_genesis_sequencer_balance + expected_reward,
        post_genesis_registry_balance,
    );
}

/// Checks that the sequencer gets rewarded the maximum priority fee if the base fee per gas is low enough.
/// This test checks the extreme case where the difference between the max fee and the base fee is strictly greater than the maximum priority fee.
/// Hence the sequencer should get rewarded the maximum priority fee.
#[test]
fn test_sequencer_rewarded_max_priority_fee() {
    // Build a STF blueprint with the module configurations
    let mut rollup = TestRollup::new();

    // The max fee is the same as the base fee so the sequencer should not get rewarded
    let base_fee =
        <S as Spec>::Gas::from_slice(&GAS_TX_FIXED_COST).value(&rollup.initial_base_fee_per_gas());

    let max_tip = TEST_PRIORITY_FEE
        .apply(base_fee)
        .expect("Should not overflow");

    let max_fee = base_fee + 5 * max_tip;

    test_sequencer_reward_in_stf(&mut rollup, max_fee, NUM_TXS_PER_BATCH * max_tip);
}

/// Checks the EIP-1559 specification for the maximum priority fee. The sequencer should get rewarded the minimum
/// of (the difference between the max fee and the base fee) and (the maximum priority fee). If the base fee is
/// very close to the max fee, the sequencer should then get rewarded less than the maximum priority fee.
/// This test checks the extreme case where the consumed base fee is the same as the max fee, hence the sequencer doesn't get rewarded at all.
#[test]
fn test_sequencer_not_rewarded_max_priority_fee() {
    // Build a STF blueprint with the module configurations
    let mut rollup = TestRollup::new();
    // The max fee is the same as the base fee so the sequencer should not get rewarded
    let max_fee =
        <S as Spec>::Gas::from_slice(&GAS_TX_FIXED_COST).value(&rollup.initial_base_fee_per_gas());

    test_sequencer_reward_in_stf(&mut rollup, max_fee, 0);
}
