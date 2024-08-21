use sov_bank::{Bank, GAS_TOKEN_ID};
use sov_chain_state::ChainState;
use sov_mock_da::MockDaSpec;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::transaction::PriorityFeeBips;
use sov_modules_api::{Gas, GasArray, Spec};
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{AsUser, SkippedReason, TransactionTestCase};

use super::helpers::S;
use crate::helpers::{setup, TestRoles, RT};

const VALUE_SETTER_NEW_CONST: u32 = 10;
const OTHER_VALUE_SETTER_CONST: u32 = 42;

/// Initialize the reward mechanism tests, and executes an empty slot to know how much gas is consumed by a simple value setter transaction.
fn reward_mechanism_test_setup() -> (TestRoles, u64, TestRunner<RT, S>) {
    // Genesis initialization.
    // We need to pass the large balance to make sure we have enough funds to pay for the tip and the sequencer registration
    let (test_roles, mut runner) = setup();

    let default_sequencer = &test_roles.default_sequencer;
    let admin = &test_roles.admin;

    // We first execute a normal transaction with no priority fee (ie the sequencer does not get rewarded).
    // This way we can know how much gas was consumed. Check that the sequencer balance was not updated
    let (output, _) = runner.simulate(
        admin
            .create_plain_message::<sov_value_setter::ValueSetter<S>>(
                sov_value_setter::CallMessage::SetValue(VALUE_SETTER_NEW_CONST),
            )
            .with_max_priority_fee_bips(PriorityFeeBips::ZERO),
        Some(default_sequencer.da_address),
    );

    let gas_consumed_last_tx = <<S as Spec>::Gas as GasArray>::from_slice(
        &output
            .batch_receipts
            .last()
            .unwrap()
            .tx_receipts
            .last()
            .unwrap()
            .gas_used,
    );
    let initial_gas_price = ChainState::<S, MockDaSpec>::initial_base_fee_per_gas();

    (
        test_roles,
        gas_consumed_last_tx.value(&initial_gas_price),
        runner,
    )
}

fn reward_mechanism_test(
    max_fee: u64,
    max_priority_fee: PriorityFeeBips,
    expected_reward: u64,
    roles: TestRoles,
    mut runner: TestRunner<RT, S>,
) {
    let TestRoles {
        default_sequencer,
        admin,
        ..
    } = roles;

    let default_sequencer_address = default_sequencer.user_info.address();
    let default_sequencer_balance = default_sequencer.user_info.available_gas_balance;

    runner.execute_transaction(TransactionTestCase {
        input: admin
            .create_plain_message::<sov_value_setter::ValueSetter<S>>(
                sov_value_setter::CallMessage::SetValue(OTHER_VALUE_SETTER_CONST),
            )
            .with_max_fee(max_fee)
            .with_max_priority_fee_bips(max_priority_fee),
        assert: Box::new(move |_result, state| {
            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&default_sequencer_address, GAS_TOKEN_ID, state)
                    .unwrap_infallible(),
                Some(default_sequencer_balance + expected_reward),
                "The sequencer was not rewarded the correct amount"
            );
        }),
    });
}

/// Tests that the sequencer gets rewarded some gas following the EIP-1559 rules.
/// When the `max_fee` is high enough and the batch is successfully executed, the sequencer gets the `consumed_gas * priority_fee`
#[test]
fn test_reward_sequencer_max_fee_high_enough() {
    let (roles, gas_consumed, runner) = reward_mechanism_test_setup();

    let priority_fee = PriorityFeeBips::from_percentage(10);

    let expected_reward = priority_fee.apply(gas_consumed).unwrap();
    let max_fee = gas_consumed + expected_reward;

    reward_mechanism_test(max_fee, priority_fee, expected_reward, roles, runner);
}

/// Tests that the sequencer gets rewarded some gas following the EIP-1559 rules.
/// When the `max_fee` is high enough to only pay for the transaction execution costs and the batch is successfully executed, the sequencer does
/// not get rewarded.
#[test]
fn test_reward_sequencer_max_fee_not_high_enough() {
    let (roles, gas_consumed, runner) = reward_mechanism_test_setup();

    let priority_fee = PriorityFeeBips::from_percentage(10);

    let expected_reward = 0;
    let max_fee = gas_consumed;

    reward_mechanism_test(max_fee, priority_fee, expected_reward, roles, runner);
}

/// Tests that the sequencer gets correctly penalized when it incorrectly processes a batch
/// For instance, this happens if there is no enough gas to execute a transaction in a batch.
#[test]
fn test_penalize_sequencer() {
    let (
        TestRoles {
            default_sequencer,
            admin,
            ..
        },
        mut runner,
    ) = setup();

    let default_sequencer_stake = default_sequencer.bond;
    let default_sequencer_da_address = default_sequencer.da_address;

    runner.execute_transaction(TransactionTestCase {
        input: admin
            .create_plain_message::<sov_value_setter::ValueSetter<S>>(
                sov_value_setter::CallMessage::SetValue(OTHER_VALUE_SETTER_CONST),
            )
            .with_max_fee(0),
        assert: Box::new(move |result, state| {
            match &result.outcome {
                sov_modules_api::TxEffect::Skipped(reason) => {
                    if let SkippedReason::CannotReserveGas(error_message) = reason {
                        assert!(error_message.contains("The gas to charge is greater than the funds available in the meter."), "Error message doesn't contain with the expected phrase. Got: {}", error_message);
                    } else {
                        panic!("Expected CannotReserveGas error, but got a different SkippedReason: {:?}", reason);
                    }
                },
                unexpected => panic!("Expected transaction to be skipped, but got: {:?}", unexpected),
            }

            let current_stake = sov_sequencer_registry::SequencerRegistry::<S, MockDaSpec>::default()
                .get_sender_balance(&default_sequencer_da_address, state)
                .unwrap_infallible().unwrap();
            let genesis_stake = default_sequencer_stake;

            assert!(
                current_stake < default_sequencer_stake,
                "The sequencer stake has not decreased which means he wasn't penalized: current stake {current_stake}, genesis stake {genesis_stake}"
            );
        })
    });
}
