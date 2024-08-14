use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use sov_bank::{Bank, ReserveGasErrorReason, GAS_TOKEN_ID};
use sov_mock_da::MockAddress;
use sov_modules_api::capabilities::{AllowedSequencer, FatalError};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::GasMeter;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{
    AsUser, SkippedReason, SlotTestCase, TestUser, TransactionType, TxTestCase,
    TEST_DEFAULT_USER_STAKE,
};

use crate::helpers::{
    setup, TestRoles, TestSequencerRegistry, ANOTHER_SEQUENCER_DA_ADDRESS, RT, S,
};

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
            .to_raw_tx::<RT>(&mut nonces, state);

        tx.data.pop();

        TransactionType::PreSigned(tx)
    })
}

fn helper_test_with_malformed_tx() -> (
    TransactionType<sov_value_setter::ValueSetter<S>, S>,
    TestRoles,
    TestRunner<RT, S>,
) {
    let (test_roles, mut runner) = setup();

    let admin = &test_roles.admin;

    // The sequencer can get slashed because he sent a batch containing a transaction that cannot be deserialized.
    let malformed_transaction = produce_malformed_tx(&mut runner, admin);

    (malformed_transaction, test_roles, runner)
}

/// Tests the slashing mechanism. In particular, it tests what happens to the sequencer registry module when a slashing event occurs.
#[test]
fn test_slash_sequencer() {
    let (
        malformed_transaction,
        TestRoles {
            default_sequencer, ..
        },
        mut runner,
    ) = helper_test_with_malformed_tx();

    let default_sequencer_da_address = default_sequencer.da_address;

    // Send the malformed transaction
    runner.execute_slots::<sov_value_setter::ValueSetter<S>>(vec![
        SlotTestCase::from_slashed_batch(
            vec![TxTestCase::dropped(malformed_transaction)],
            FatalError::DeserializationFailed("IO error: Unexpected length of input".to_string()),
        )
        .with_end_slot_hook(Box::new(move |state| {
            assert!(
                !TestSequencerRegistry::default()
                    .is_registered_sequencer(&default_sequencer_da_address, state)
                    .unwrap_infallible(),
                "The default sequencer should not be registered anymore"
            );
        })),
    ]);
}

/// Tests the slashing mechanism for a preferred sequencer
#[test]
fn test_slash_preferred_sequencer() {
    let (malformed_transaction, _, mut runner) = helper_test_with_malformed_tx();

    // Send the malformed transaction
    runner.execute_slots::<sov_value_setter::ValueSetter<S>>(vec![
        SlotTestCase::from_slashed_batch(
            vec![TxTestCase::dropped(malformed_transaction)],
            FatalError::DeserializationFailed("IO error: Unexpected length of input".to_string()),
        )
        .with_end_slot_hook(Box::new(move |state| {
            assert_eq!(
                TestSequencerRegistry::default()
                    .get_preferred_sequencer(state)
                    .unwrap_infallible(),
                None,
                "The preferred sequencer should not be registered anymore"
            );
        })),
    ]);
}

/// Tests that the sequencer without enough stake is not slashed
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

    let minimal_bond = runner
        .query_state(|state| {
            TestSequencerRegistry::default()
                .minimum_bond
                .get(state)
                .unwrap_infallible()
        })
        .expect("The minimum bond should be set at genesis");

    let additional_sequencer_da_address = ANOTHER_SEQUENCER_DA_ADDRESS;

    // We first register a sequencer with the minimal bond amount
    runner.execute_slots::<TestSequencerRegistry>(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<RT, _, _>::applied_with_hook(
            additional_sequencer.create_plain_message::<TestSequencerRegistry>(
                sov_sequencer_registry::CallMessage::Register {
                    da_address: additional_sequencer_da_address.as_ref().to_vec(),
                    amount: minimal_bond,
                },
            ),
            Box::new(move |state| {
                assert!(
                    TestSequencerRegistry::default()
                        .is_registered_sequencer(
                            &MockAddress::from(additional_sequencer_da_address),
                            state
                        )
                        .unwrap_infallible(),
                    "The additional sequencer should be registered"
                );
            }),
        ),
    ])]);

    let malformed_transaction = produce_malformed_tx(&mut runner, &admin);

    // First, we send a transaction with max fee 0. Since the tx doesn't provide enough fees to cover
    // the cost of its deserialization, the sequencer pays the difference. This reduces his balance below
    // the minimum.
    //
    // Next we send a malformed transaction. Since the sequencer's balance is below the minimum, the transaction
    // is ignored. This means that the sequencer is *not* slashed even though the transaction is malicious.
    runner.execute_slots::<sov_value_setter::ValueSetter<S>>(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<RT, _, _>::skipped(
            admin
                .create_plain_message::<sov_value_setter::ValueSetter<S>>(
                    sov_value_setter::CallMessage::SetValue(10),
                )
                .with_max_fee(0),
            SkippedReason::CannotReserveGas(
                ReserveGasErrorReason::<S>::InsufficientGasForPreExecutionChecks(
                    "The gas to charge is greater than the funds available in the meter. Gas to charge GasUnit[2261, 2261], gas price GasPrice[9, 9], remaining funds 0, total gas consumed GasUnit[0, 0]".to_string()
                )
                .to_string(),
            ))]),
            SlotTestCase::from_dropped_batch(vec![TxTestCase::dropped(malformed_transaction)]).with_end_slot_hook(Box::new(move |state| {
                assert!(
                    TestSequencerRegistry::default()
                        .is_registered_sequencer(&additional_sequencer_da_address.into(), state)
                        .unwrap_infallible(),
                    "The additional sequencer should still be registered"
                );
            }
            ))
            ]);
}

/// When a sequencer is slashed, the slashed tokens do not reappear on the sequencer's account, and are not accessible after registration.
#[test]
fn slashed_sequencer_should_not_preserve_balance() {
    let (
        malformed_transaction,
        TestRoles {
            additional_sequencer,
            ..
        },
        mut runner,
    ) = helper_test_with_malformed_tx();

    let additional_sequencer_da_address = ANOTHER_SEQUENCER_DA_ADDRESS;
    let additional_sequencer_address = additional_sequencer.address();
    let additional_sequencer_balance = additional_sequencer.available_balance;

    let gas_consumed_registration_ref = Arc::new(AtomicU64::new(0));
    let gas_consumed_registration_ref_1 = gas_consumed_registration_ref.clone();

    // Register the additional sequencer
    runner.execute_slots::<TestSequencerRegistry>(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<RT, _, _>::applied_with_hook(
            additional_sequencer.create_plain_message::<TestSequencerRegistry>(
                sov_sequencer_registry::CallMessage::Register {
                    da_address: additional_sequencer_da_address.as_ref().to_vec(),
                    amount: TEST_DEFAULT_USER_STAKE,
                },
            ),
            Box::new(move |state| {
                assert_eq!(
                    TestSequencerRegistry::default()
                        .is_sender_allowed(&additional_sequencer_da_address.into(), state)
                        .unwrap(),
                    AllowedSequencer {
                        address: additional_sequencer_address,
                        balance: TEST_DEFAULT_USER_STAKE,
                    },
                    "The additional sequencer should be registered"
                );

                gas_consumed_registration_ref.fetch_add(
                    state.inner().gas_used_value(),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }),
        ),
    ])]);

    // Send the malformed transaction
    runner.execute_slots::<sov_value_setter::ValueSetter<S>>(vec![
        SlotTestCase::from_slashed_batch(
            vec![TxTestCase::dropped(malformed_transaction)],
            FatalError::DeserializationFailed("IO error: Unexpected length of input".to_string()),
        )
        .with_end_slot_hook(Box::new(move |state| {
            assert!(
                !TestSequencerRegistry::default()
                    .is_registered_sequencer(&additional_sequencer_da_address.into(), state)
                    .unwrap_infallible(),
                "The default sequencer should not be registered anymore"
            );

            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&additional_sequencer_address, GAS_TOKEN_ID, state)
                    .unwrap_infallible(),
                Some(
                    additional_sequencer_balance
                        - gas_consumed_registration_ref_1.load(std::sync::atomic::Ordering::SeqCst)
                        - TEST_DEFAULT_USER_STAKE
                ),
                "The sequencer's balance should be equal to the initial balance minus the gas used to register + the stake amount"
            );
        }))
        .with_sequencer(additional_sequencer_da_address.into()),
    ]);

    // Try to register the sequencer again
    runner.execute_slots::<TestSequencerRegistry>(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<RT, _, _>::applied_with_hook(
            additional_sequencer.create_plain_message::<TestSequencerRegistry>(
                sov_sequencer_registry::CallMessage::Register {
                    da_address: additional_sequencer_da_address.as_ref().to_vec(),
                    // We try to register the sequencer with a new stake amount that is not a multiple of the
                    // previous stake amount to ensure that the stake amount is not accumulated.
                    amount: 3 * TEST_DEFAULT_USER_STAKE / 2,
                },
            ),
            Box::new(move |state| {
                assert_eq!(
                    TestSequencerRegistry::default()
                        .is_sender_allowed(&additional_sequencer_da_address.into(), state)
                        .unwrap(),
                    AllowedSequencer {
                        address: additional_sequencer_address,
                        balance: 3 * TEST_DEFAULT_USER_STAKE / 2,
                    },
                    "The additional sequencer should be registered"
                );
            }),
        ),
    ])]);
}
