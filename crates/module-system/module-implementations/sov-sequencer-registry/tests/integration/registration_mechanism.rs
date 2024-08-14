use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use sov_mock_da::MockAddress;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::transaction::PriorityFeeBips;
use sov_modules_api::Error::ModuleError;
use sov_modules_api::GasMeter;
use sov_sequencer_registry::{CallMessage, CustomError};
use sov_test_utils::runtime::{TestRunner, ValueSetter};
use sov_test_utils::{
    AsUser, SlotTestCase, TxTestCase, TEST_DEFAULT_USER_BALANCE, TEST_DEFAULT_USER_STAKE,
};

use crate::helpers::{
    setup, TestRoles, TestSequencerRegistry, TestSequencerRegistryError,
    ANOTHER_SEQUENCER_DA_ADDRESS, NON_DEFAULT_SEQUENCER_DA_ADDRESS, RT,
};

type S = sov_test_utils::TestSpec;

// Happy path for registration and exit.
// This test checks:
//  - genesis sequencer is present after genesis
//  - he can process transactions
#[test]
fn test_default_sequencer() {
    let (
        TestRoles {
            default_sequencer: test_sequencer,
            admin,
            ..
        },
        mut runner,
    ) = setup();

    let test_sequencer_address = test_sequencer.user_info.address();
    let test_sequencer_initial_balance = test_sequencer.user_info.available_balance;

    let custom_priority_fee = PriorityFeeBips::from_percentage(10);

    let gas_consumed_ref = Arc::new(AtomicU64::new(0));
    let gas_consumed_ref_1 = gas_consumed_ref.clone();

    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<RT, _, _>::applied_with_hook(
            admin
                .create_plain_message::<ValueSetter<S>>(sov_value_setter::CallMessage::SetValue(10))
                .with_max_priority_fee_bips(custom_priority_fee),
            Box::new(move |state| {
                gas_consumed_ref.fetch_add(
                    state.inner().gas_used_value(),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }),
        ),
    ])
    .with_end_slot_hook(Box::new(move |state| {
        // Assert that the sequencer has been rewarded
        assert_eq!(
            TestRunner::<RT, S>::bank_gas_balance(&test_sequencer_address, state),
            Some(
                test_sequencer_initial_balance
                    + custom_priority_fee
                        .apply(gas_consumed_ref_1.load(std::sync::atomic::Ordering::SeqCst))
                        .unwrap()
            ),
            "The sequencer should have been rewarded the execution funds "
        );
    }))]);
}

#[test]
fn test_new_sequencer_registration() {
    let (
        TestRoles {
            additional_sequencer,
            admin,
            ..
        },
        mut runner,
    ) = setup();

    let other_sequencer_address = additional_sequencer.address();
    let other_sequencer_da_address = MockAddress::new(NON_DEFAULT_SEQUENCER_DA_ADDRESS);

    let gas_used_ref = Arc::new(AtomicU64::new(0));
    let gas_used_ref_1 = gas_used_ref.clone();

    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<RT, _, _>::applied_with_hook(
            additional_sequencer.create_plain_message::<TestSequencerRegistry>(
                sov_sequencer_registry::CallMessage::Register {
                    da_address: other_sequencer_da_address.as_ref().to_vec(),
                    amount: TEST_DEFAULT_USER_STAKE,
                },
            ),
            Box::new(move |state| {
                // Assert that the sequencer has correctly been registered
                assert!(
                    TestSequencerRegistry::default()
                        .is_sender_allowed(
                            &MockAddress::new(NON_DEFAULT_SEQUENCER_DA_ADDRESS),
                            state
                        )
                        .is_ok(),
                    "The sequencer is not registered"
                );

                // Assert that a registration event has been emitted
                assert!(state.inner().events().iter().any(|event| {
                    event.downcast_ref::<sov_sequencer_registry::Event<S>>()
                        == Some(&sov_sequencer_registry::Event::Registered {
                            sequencer: other_sequencer_address,
                            amount: TEST_DEFAULT_USER_STAKE,
                        })
                }));

                gas_used_ref.fetch_add(
                    state.inner().gas_used_value(),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }),
        ),
    ])
    .with_end_slot_hook(Box::new(move |state| {
        // Assert that the other sequencer balance has been updated
        assert_eq!(
            TestRunner::<RT, S>::bank_gas_balance(&other_sequencer_address, state),
            Some(
                TEST_DEFAULT_USER_BALANCE
                    - TEST_DEFAULT_USER_STAKE
                    - gas_used_ref_1.load(std::sync::atomic::Ordering::SeqCst)
            )
        );
    }))]);

    // The new sequencer should be allowed to process transactions
    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::applied(
            admin.create_plain_message::<ValueSetter<S>>(sov_value_setter::CallMessage::SetValue(
                10,
            )),
        ),
    ])
    .with_sequencer(other_sequencer_da_address)]);
}

#[test]
fn test_registration_not_enough_funds() {
    let (
        TestRoles {
            additional_sequencer,
            ..
        },
        mut runner,
    ) = setup();

    let other_sequencer_balance = additional_sequencer.available_balance;

    let additional_sequencer_address = additional_sequencer.address();

    let amount_to_register = other_sequencer_balance + TEST_DEFAULT_USER_STAKE;

    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::reverted(
            additional_sequencer.create_plain_message::<TestSequencerRegistry>(
                sov_sequencer_registry::CallMessage::Register {
                    da_address: NON_DEFAULT_SEQUENCER_DA_ADDRESS.to_vec(),
                    amount: amount_to_register,
                },
            ),
            ModuleError(
                TestSequencerRegistryError::InsufficientFundsToRegister {
                    address: additional_sequencer_address,
                    amount: amount_to_register,
                }
                .into(),
            ),
        ),
    ])]);
}

#[test]
fn test_registration_second_time() {
    let (
        TestRoles {
            additional_sequencer,
            ..
        },
        mut runner,
    ) = setup();

    let other_sequencer_address = additional_sequencer.address();

    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::applied(
            additional_sequencer.create_plain_message::<TestSequencerRegistry>(
                sov_sequencer_registry::CallMessage::Register {
                    da_address: NON_DEFAULT_SEQUENCER_DA_ADDRESS.to_vec(),
                    amount: TEST_DEFAULT_USER_STAKE,
                },
            ),
        ),
    ])]);

    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::reverted(
            additional_sequencer.create_plain_message::<TestSequencerRegistry>(
                sov_sequencer_registry::CallMessage::Register {
                    da_address: NON_DEFAULT_SEQUENCER_DA_ADDRESS.to_vec(),
                    amount: TEST_DEFAULT_USER_STAKE,
                },
            ),
            ModuleError(
                TestSequencerRegistryError::AlreadyRegistered(other_sequencer_address).into(),
            ),
        ),
    ])]);
}

/// Tests that an other sequencer can register and exit.
#[test]
fn test_exit_happy_path() {
    let (roles, mut runner) = setup();

    let additional_sequencer = roles.additional_sequencer;

    let other_sequencer_address = additional_sequencer.address();
    let other_sequencer_da_address = MockAddress::new(NON_DEFAULT_SEQUENCER_DA_ADDRESS);

    let other_sequencer_balance_ref =
        Arc::new(AtomicU64::new(additional_sequencer.available_balance));
    let other_sequencer_balance_ref_1 = other_sequencer_balance_ref.clone();
    let other_sequencer_balance_ref_2 = other_sequencer_balance_ref.clone();
    let other_sequencer_balance_ref_3 = other_sequencer_balance_ref.clone();

    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<RT, _, _>::applied_with_hook(
            additional_sequencer.create_plain_message::<TestSequencerRegistry>(
                sov_sequencer_registry::CallMessage::Register {
                    da_address: other_sequencer_da_address.as_ref().to_vec(),
                    amount: TEST_DEFAULT_USER_STAKE,
                },
            ),
            Box::new(move |state| {
                // Assert that the sequencer has correctly been registered
                assert!(
                    TestSequencerRegistry::default()
                        .is_sender_allowed(
                            &MockAddress::new(NON_DEFAULT_SEQUENCER_DA_ADDRESS),
                            state
                        )
                        .is_ok(),
                    "The sequencer is not registered"
                );

                // Update the other sequencer's balance
                other_sequencer_balance_ref.fetch_sub(
                    state.inner().gas_used_value(),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }),
        ),
    ])
    .with_end_slot_hook(Box::new(move |state| {
        // Assert that the other sequencer balance has been updated
        assert_eq!(
            TestRunner::<RT, S>::bank_gas_balance(&other_sequencer_address, state),
            Some(
                other_sequencer_balance_ref_1.load(std::sync::atomic::Ordering::SeqCst)
                    - TEST_DEFAULT_USER_STAKE
            )
        );
    }))]);

    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<RT, _, _>::applied_with_hook(
            additional_sequencer.create_plain_message::<TestSequencerRegistry>(
                sov_sequencer_registry::CallMessage::Exit {
                    da_address: other_sequencer_da_address.as_ref().to_vec(),
                },
            ),
            Box::new(move |state| {
                // Assert that the sequencer has correctly been unregistered
                assert!(
                    TestSequencerRegistry::default()
                        .is_sender_allowed(
                            &MockAddress::new(NON_DEFAULT_SEQUENCER_DA_ADDRESS),
                            state
                        )
                        .is_err(),
                    "The sequencer should be registered"
                );

                // Assert that an exit event has been emitted
                assert!(state.inner().events().iter().any(|event| {
                    event.downcast_ref::<sov_sequencer_registry::Event<S>>()
                        == Some(&sov_sequencer_registry::Event::Exited {
                            sequencer: other_sequencer_address,
                            amount_withdrawn: 100000000,
                        })
                }));

                // Update the other sequencer's balance
                other_sequencer_balance_ref_2.fetch_sub(
                    state.inner().gas_used_value(),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }),
        ),
    ])
    .with_end_slot_hook(Box::new(move |state| {
        // Assert that the other sequencer balance has been updated and that he recovered his bond
        assert_eq!(
            TestRunner::<RT, S>::bank_gas_balance(&other_sequencer_address, state),
            Some(other_sequencer_balance_ref_3.load(std::sync::atomic::Ordering::SeqCst))
        );
    }))]);
}

/// Tests that an other sequencer cannot exit in their own batch.
#[test]
fn cannot_exit_with_own_batch() {
    let (
        TestRoles {
            default_sequencer, ..
        },
        mut runner,
    ) = setup();

    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![TxTestCase::<
        RT,
        _,
        _,
    >::reverted(
        default_sequencer.create_plain_message::<TestSequencerRegistry>(CallMessage::Exit {
            da_address: default_sequencer.da_address.as_ref().to_vec(),
        }),
        ModuleError(
            TestSequencerRegistryError::Custom(CustomError::CannotUnregisterDuringOwnBatch(
                default_sequencer.da_address,
            ))
            .into(),
        ),
    )])]);
}

/// Test that triggers the [`SequencerRegistryError::SuppliedAddressDoesNotMatchTxSender`] error by sending an exit transaction from a different sender.
#[test]
fn test_exit_different_sender_fails() {
    let (
        TestRoles {
            additional_sequencer,
            admin: second_sequencer,
            ..
        },
        mut runner,
    ) = setup();

    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::applied(
            additional_sequencer.create_plain_message::<TestSequencerRegistry>(
                sov_sequencer_registry::CallMessage::Register {
                    da_address: NON_DEFAULT_SEQUENCER_DA_ADDRESS.to_vec(),
                    amount: TEST_DEFAULT_USER_STAKE,
                },
            ),
        ),
        TxTestCase::applied(
            second_sequencer.create_plain_message::<TestSequencerRegistry>(
                sov_sequencer_registry::CallMessage::Register {
                    da_address: ANOTHER_SEQUENCER_DA_ADDRESS.to_vec(),
                    amount: TEST_DEFAULT_USER_STAKE,
                },
            ),
        ),
        TxTestCase::reverted(
            additional_sequencer.create_plain_message::<TestSequencerRegistry>(
                sov_sequencer_registry::CallMessage::Exit {
                    da_address: ANOTHER_SEQUENCER_DA_ADDRESS.to_vec(),
                },
            ),
            ModuleError(
                TestSequencerRegistryError::Custom(
                    CustomError::SuppliedAddressDoesNotMatchTxSender {
                        parameter: second_sequencer.address(),
                        sender: additional_sequencer.address(),
                    },
                )
                .into(),
            ),
        ),
    ])]);
}

/// By default the genesis sequencer is also the preferred sequencer. This checks whether the preferred sequencer is returned correctly.
#[test]
fn test_get_preferred_sequencer() {
    let (
        TestRoles {
            default_sequencer, ..
        },
        mut runner,
    ) = setup();

    runner.query_state(|state| {
        assert_eq!(
            Some(default_sequencer.da_address),
            TestSequencerRegistry::default()
                .get_preferred_sequencer(state)
                .unwrap_infallible()
        );
    });
}

/// Tests that the preferred sequencer can exit and is removed from the list of preferred sequencers.
#[test]
fn test_get_preferred_sequencer_after_exit() {
    let (
        TestRoles {
            default_sequencer,
            additional_sequencer,
            ..
        },
        mut runner,
    ) = setup();

    let additional_sequencer_da_address = ANOTHER_SEQUENCER_DA_ADDRESS;

    // Register the additional sequencer
    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::applied(
            additional_sequencer.create_plain_message::<TestSequencerRegistry>(
                sov_sequencer_registry::CallMessage::Register {
                    da_address: ANOTHER_SEQUENCER_DA_ADDRESS.as_ref().to_vec(),
                    amount: TEST_DEFAULT_USER_STAKE,
                },
            ),
        ),
    ])]);

    // Then exit the normal sequencer
    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::applied(
            default_sequencer.create_plain_message::<TestSequencerRegistry>(
                sov_sequencer_registry::CallMessage::Exit {
                    da_address: default_sequencer.da_address.as_ref().to_vec(),
                },
            ),
        ),
    ])
    .with_sequencer(additional_sequencer_da_address.into())]);

    // Check that the normal sequencer is no longer the preferred sequencer
    runner.query_state(|state| {
        assert_eq!(
            None,
            TestSequencerRegistry::default()
                .get_preferred_sequencer(state)
                .unwrap_infallible()
        );
    });
}

/// Tests that increasing the stake amount fails if sequencer does not have enough funds.
#[test]
fn test_balance_increase_fails_if_insufficient_funds() {
    let (
        TestRoles {
            default_sequencer, ..
        },
        mut runner,
    ) = setup();

    let default_sequencer_balance = default_sequencer.user_info.available_balance;
    let default_sequencer_address = default_sequencer.user_info.address();

    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::reverted(
            default_sequencer.create_plain_message::<TestSequencerRegistry>(
                sov_sequencer_registry::CallMessage::Deposit {
                    da_address: default_sequencer.da_address.as_ref().to_vec(),
                    amount: default_sequencer_balance + TEST_DEFAULT_USER_STAKE,
                },
            ),
            ModuleError(
                TestSequencerRegistryError::InsufficientFundsToTopUpAccount {
                    address: default_sequencer_address,
                    amount_to_add: default_sequencer_balance + TEST_DEFAULT_USER_STAKE,
                }
                .into(),
            ),
        ),
    ])]);
}

#[test]
fn test_non_registered_sequencer_is_not_allowed() {
    let (_, mut runner) = setup();

    runner.query_state(|state| {
        assert!(
            TestSequencerRegistry::default()
                .is_sender_allowed(&MockAddress::from(NON_DEFAULT_SEQUENCER_DA_ADDRESS), state)
                .is_err(),
            "Non-registered sequencers should not be allowed"
        );
    });
}

/// We should not be able to increase the stake amount (through deposit) for a non-registered sequencer.
#[test]
fn test_balance_increase_fails_for_unknown_sequencer() {
    let (
        TestRoles {
            additional_sequencer,
            ..
        },
        mut runner,
    ) = setup();

    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::reverted(
            additional_sequencer.create_plain_message::<TestSequencerRegistry>(
                sov_sequencer_registry::CallMessage::Deposit {
                    da_address: NON_DEFAULT_SEQUENCER_DA_ADDRESS.as_ref().to_vec(),
                    amount: TEST_DEFAULT_USER_STAKE,
                },
            ),
            ModuleError(
                TestSequencerRegistryError::IsNotRegistered(MockAddress::from(
                    NON_DEFAULT_SEQUENCER_DA_ADDRESS,
                ))
                .into(),
            ),
        ),
    ])]);
}
