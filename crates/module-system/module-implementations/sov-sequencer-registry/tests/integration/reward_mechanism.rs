use sov_bank::utils::TokenHolder;
use sov_bank::{config_gas_token_id, Bank};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::transaction::PriorityFeeBips;
use sov_modules_api::{Gas, GasSpec, ModuleInfo};
use sov_sequencer_registry::SequencerRegistry;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{
    get_gas_used, AsUser, BatchTestCase, TestUser, TransactionTestCase, TransactionType,
    TxProcessingError,
};

use super::helpers::S;
use crate::helpers::{setup, TestRoles, TestSequencerRegistry, ANOTHER_SEQUENCER_DA_ADDRESS, RT};

const VALUE_SETTER_NEW_CONST: u32 = 10;
const OTHER_VALUE_SETTER_CONST: u32 = 42;

/// Initialize the reward mechanism tests, and executes an empty slot to know how much gas is consumed by a simple value setter transaction.
fn reward_mechanism_test_setup() -> (TestRoles, u64, TestRunner<RT, S>) {
    // Genesis initialization.
    // We need to pass the large balance to make sure we have enough funds to pay for the tip and the sequencer registration
    let (test_roles, mut runner) = setup();

    let default_sequencer = &test_roles.default_sequencer;
    let admin = &test_roles.admin;

    runner.config.sequencer_da_address = default_sequencer.da_address;
    // We first execute a normal transaction with no priority fee (ie the sequencer does not get rewarded).
    // This way we can know how much gas was consumed. Check that the sequencer balance was not updated
    let (output, _) = runner.simulate(
        admin
            .create_plain_message::<sov_value_setter::ValueSetter<S>>(
                sov_value_setter::CallMessage::SetValue(VALUE_SETTER_NEW_CONST),
            )
            .with_max_priority_fee_bips(PriorityFeeBips::ZERO),
    );

    let gas_consumed_last_tx = get_gas_used(
        output
            .batch_receipts
            .last()
            .unwrap()
            .tx_receipts
            .last()
            .unwrap(),
    );
    let initial_gas_price = S::initial_base_fee_per_gas();

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
    runner: &mut TestRunner<RT, S>,
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
                    .get_balance_of(&default_sequencer_address, config_gas_token_id(), state)
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
    let (roles, gas_consumed, mut runner) = reward_mechanism_test_setup();

    let priority_fee = PriorityFeeBips::from_percentage(10);

    let expected_reward = priority_fee.apply(gas_consumed).unwrap();
    let max_fee = gas_consumed + expected_reward;

    reward_mechanism_test(max_fee, priority_fee, expected_reward, roles, &mut runner);
}

/// Tests that the sequencer gets rewarded some gas following the EIP-1559 rules.
/// When the `max_fee` is high enough to only pay for the transaction execution costs and the batch is successfully executed, the sequencer does
/// not get rewarded.
#[test]
fn test_reward_sequencer_max_fee_not_high_enough() {
    let (roles, gas_consumed, mut runner) = reward_mechanism_test_setup();

    let priority_fee = PriorityFeeBips::from_percentage(10);

    let expected_reward = 0;
    let max_fee = gas_consumed;

    reward_mechanism_test(max_fee, priority_fee, expected_reward, roles, &mut runner);
}

/// Tests that the sequencer registry balance does not change after rewarding the sequencer.
/// If the balance changed the sequencer registry would break because it will eventually run out of funds.
/// This is a regression test for `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/pull/1466>`.
#[test]
fn test_reward_sequencer_registry_balance_does_not_change() {
    let (roles, gas_consumed, mut runner) = reward_mechanism_test_setup();

    let sequencer_registry_balance = |runner: &TestRunner<RT, S>| {
        runner.query_state(|state| {
            let sequencer_id = *SequencerRegistry::<S>::default().id();

            Bank::<S>::default()
                .get_balance_of(
                    TokenHolder::Module(sequencer_id),
                    config_gas_token_id(),
                    state,
                )
                .unwrap_infallible()
        })
    };

    let priority_fee = PriorityFeeBips::from_percentage(10);

    let expected_reward = priority_fee.apply(gas_consumed).unwrap();
    let max_fee = gas_consumed + expected_reward;

    let balance_before = sequencer_registry_balance(&runner);

    reward_mechanism_test(max_fee, priority_fee, expected_reward, roles, &mut runner);

    let balance_after = sequencer_registry_balance(&runner);

    assert_eq!(
        balance_before, balance_after,
        "The sequencer registry balance should not change after rewarding the sequencer"
    );
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
            match &result.tx_receipt {
                sov_modules_api::TxEffect::Skipped(skipped) => {
                    if let TxProcessingError::OutOfGas(error_message) = &skipped.error {
                        assert!(error_message.contains("The gas to charge is greater than the funds available in the meter."), "Error message doesn't contain with the expected phrase. Got: {}", error_message);
                    } else {
                        panic!("Expected CannotReserveGas error, but got a different SkippedReason: {:?}", skipped.error);
                    }
                },
                unexpected => panic!("Expected transaction to be skipped, but got: {:?}", unexpected),
            }

            let current_stake = sov_sequencer_registry::SequencerRegistry::<S>::default()
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

fn produce_malformed_tx(
    runner: &mut TestRunner<RT, S>,
    admin: &TestUser<S>,
) -> TransactionType<sov_value_setter::ValueSetter<S>, S> {
    let mut nonces = runner.nonces().clone();

    runner.query_state(|state| {
        let mut tx = admin
            .create_plain_message::<sov_value_setter::ValueSetter<S>>(
                sov_value_setter::CallMessage::SetValue(10),
            )
            .to_serialized_authenticated_tx::<RT>(&mut nonces, state);

        tx.data.pop();

        TransactionType::PreAuthenticated(tx)
    })
}

#[test]
fn test_sequencer_without_enough_stake() {
    let (
        TestRoles {
            additional_sequencer,
            admin,
            ..
        },
        mut runner,
    ) = setup();

    let minimal_bond = runner.query_state(|state| {
        TestSequencerRegistry::default()
            .get_coins_to_lock(state)
            .unwrap_infallible()
            .amount
    });

    let additional_sequencer_da_address = ANOTHER_SEQUENCER_DA_ADDRESS;

    // We first register a sequencer with the minimal bond amount
    let register_tx = additional_sequencer.create_plain_message::<TestSequencerRegistry>(
        sov_sequencer_registry::CallMessage::Register {
            da_address: additional_sequencer_da_address.as_ref().to_vec(),
            amount: minimal_bond,
        },
    );

    runner.execute(register_tx);

    let malformed_transaction = produce_malformed_tx(&mut runner, &admin);

    // First, we send a transaction with max fee 0. Since the tx doesn't provide enough fees to cover
    // the cost of its deserialization, the sequencer pays the difference. This reduces his balance below
    // the minimum.
    //
    // Next we send a malformed transaction. Since the sequencer's balance is below the minimum, the transaction
    // is ignored. This means that the sequencer is *not* slashed even though the transaction is malicious.
    runner.execute_batch(BatchTestCase {
        input: vec![
            admin
                .create_plain_message::<sov_value_setter::ValueSetter<S>>(
                    sov_value_setter::CallMessage::SetValue(10),
                )
                .with_max_fee(0),
            malformed_transaction,
        ]
        .into(),
        assert: Box::new(move |result, state| {
            assert_eq!(
                result.batch_receipt.clone().unwrap().tx_receipts.len(), 1,
                "Only the first transaction should have been included in the batch"
            );

            let tx_receipt = &result.batch_receipt.unwrap().tx_receipts[0];

            match &tx_receipt.receipt {
                sov_modules_api::TxEffect::Skipped(skipped) => {
                    if let TxProcessingError::OutOfGas(error_message) = &skipped.error {
                        assert!(
                            error_message.contains("The gas to charge is greater than the funds available in the meter."),
                            "Error message doesn't contain with the expected phrase. Got: {}",
                            error_message
                        );
                    } else {
                        panic!("Expected CannotReserveGas error, but got a different SkippedReason: {:?}", skipped.error);
                    }
                },
                unexpected => panic!("Expected transaction to revert, but got: {:?}", unexpected),
            };

            assert!(
                TestSequencerRegistry::default()
                    .is_registered_sequencer(&additional_sequencer_da_address.into(), state)
                    .unwrap_infallible(),
                "The additional sequencer should still be registered"
            );
        }),
    });
}
