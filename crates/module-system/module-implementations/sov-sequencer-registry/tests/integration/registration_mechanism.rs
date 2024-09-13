use sov_mock_da::MockAddress;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::transaction::PriorityFeeBips;
use sov_modules_api::Error::ModuleError;
use sov_modules_api::TxEffect;
use sov_sequencer_registry::{CallMessage, CustomError};
use sov_test_utils::runtime::{TestRunner, ValueSetter};
use sov_test_utils::{
    AsUser, AtomicNumber, BatchTestCase, BatchType, TransactionTestCase, TEST_DEFAULT_USER_BALANCE,
    TEST_DEFAULT_USER_STAKE,
};

use crate::helpers::{
    setup, TestRoles, TestRuntimeEvent, TestSequencerRegistry, TestSequencerRegistryError,
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
    let test_sequencer_da_address = test_sequencer.da_address;
    let test_sequencer_initial_balance = test_sequencer.user_info.available_gas_balance;

    let custom_priority_fee = PriorityFeeBips::from_percentage(10);

    runner.execute_transaction(TransactionTestCase {
        input: admin
            .create_plain_message::<ValueSetter<S>>(sov_value_setter::CallMessage::SetValue(10))
            .with_max_priority_fee_bips(custom_priority_fee),
        assert: Box::new(move |result, state| {
            // Assert that the sequencer has been rewarded
            assert_eq!(
                TestRunner::<RT, S>::bank_gas_balance(&test_sequencer_address, state),
                Some(
                    test_sequencer_initial_balance
                        + custom_priority_fee.apply(result.gas_value_used).unwrap()
                ),
                "The sequencer should have been rewarded the execution funds "
            );

            assert_eq!(
                TestSequencerRegistry::default()
                    .resolve_da_address(&test_sequencer_da_address, state)
                    .unwrap_infallible(),
                Some(test_sequencer_address)
            );
        }),
    });
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

    runner.execute_transaction(TransactionTestCase {
        input: additional_sequencer.create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Register {
                da_address: other_sequencer_da_address.as_ref().to_vec(),
                amount: TEST_DEFAULT_USER_STAKE,
            },
        ),
        assert: Box::new(move |result, state| {
            // Assert that the sequencer has correctly been registered
            assert!(
                TestSequencerRegistry::default()
                    .is_sender_allowed(&MockAddress::new(NON_DEFAULT_SEQUENCER_DA_ADDRESS), state)
                    .is_ok(),
                "The sequencer is not registered"
            );
            // Assert that a registration event has been emitted
            assert!(result.events.iter().any(|event| matches!(
                event,
                TestRuntimeEvent::SequencerRegistry(
                    sov_sequencer_registry::Event::Registered { sequencer, amount }
                ) if *sequencer == other_sequencer_address && *amount == TEST_DEFAULT_USER_STAKE
            )));
            // Assert that the other sequencer balance has been updated
            assert_eq!(
                TestRunner::<RT, S>::bank_gas_balance(&other_sequencer_address, state),
                Some(TEST_DEFAULT_USER_BALANCE - TEST_DEFAULT_USER_STAKE - result.gas_value_used)
            );
        }),
    });

    runner.config.sequencer_da_address = other_sequencer_da_address;

    runner.execute_batch(BatchTestCase {
        input: vec![admin
            .create_plain_message::<ValueSetter<S>>(sov_value_setter::CallMessage::SetValue(10))]
        .into(),
        assert: Box::new(move |result, _state| {
            assert!(result.batch_receipt.unwrap().tx_receipts[0]
                .receipt
                .is_successful());
        }),
    });
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

    let other_sequencer_balance = additional_sequencer.available_gas_balance;

    let additional_sequencer_address = additional_sequencer.address();

    let amount_to_register = other_sequencer_balance + TEST_DEFAULT_USER_STAKE;

    runner.execute_transaction(TransactionTestCase {
        input: additional_sequencer.create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Register {
                da_address: NON_DEFAULT_SEQUENCER_DA_ADDRESS.to_vec(),
                amount: amount_to_register,
            },
        ),
        assert: Box::new(move |result, _state| match &result.tx_receipt {
            TxEffect::Reverted(reason) => {
                assert_eq!(
                    reason.reason,
                    ModuleError(
                        TestSequencerRegistryError::InsufficientFundsToRegister {
                            address: additional_sequencer_address,
                            amount: amount_to_register,
                        }
                        .into(),
                    ),
                    "Transaction reverted, but with unexpected reason"
                );
            }
            unexpected => panic!("Expected transaction to revert, but got: {:?}", unexpected),
        }),
    });
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

    runner.execute(
        additional_sequencer.create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Register {
                da_address: NON_DEFAULT_SEQUENCER_DA_ADDRESS.to_vec(),
                amount: TEST_DEFAULT_USER_STAKE,
            },
        ),
    );

    runner.execute_transaction(TransactionTestCase {
        input: additional_sequencer.create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Register {
                da_address: NON_DEFAULT_SEQUENCER_DA_ADDRESS.to_vec(),
                amount: TEST_DEFAULT_USER_STAKE,
            },
        ),
        assert: Box::new(move |result, _state| match &result.tx_receipt {
            TxEffect::Reverted(reason) => {
                assert_eq!(
                    reason.reason,
                    ModuleError(
                        TestSequencerRegistryError::AlreadyRegistered(other_sequencer_address)
                            .into(),
                    ),
                    "Transaction reverted, but with unexpected reason"
                );
            }
            unexpected => panic!("Expected transaction to revert, but got: {:?}", unexpected),
        }),
    });
}

/// Tests that an other sequencer can register and exit.
#[test]
fn test_exit_happy_path() {
    let (roles, mut runner) = setup();

    let additional_sequencer = roles.additional_sequencer;

    let other_sequencer_address = additional_sequencer.address();
    let other_sequencer_da_address = MockAddress::new(NON_DEFAULT_SEQUENCER_DA_ADDRESS);

    let other_sequencer_balance_ref = AtomicNumber::new(additional_sequencer.available_gas_balance);
    let other_sequencer_balance_ref_1 = other_sequencer_balance_ref.clone();

    let register = TransactionTestCase {
        input: additional_sequencer.create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Register {
                da_address: other_sequencer_da_address.as_ref().to_vec(),
                amount: TEST_DEFAULT_USER_STAKE,
            },
        ),
        assert: Box::new(move |result, state| {
            // Assert that the sequencer has correctly been registered
            assert!(
                TestSequencerRegistry::default()
                    .is_sender_allowed(&MockAddress::new(NON_DEFAULT_SEQUENCER_DA_ADDRESS), state)
                    .is_ok(),
                "The sequencer is not registered"
            );
            // Update the other sequencer's balance
            other_sequencer_balance_ref.sub(result.gas_value_used);
            // Assert that the other sequencer balance has been updated
            assert_eq!(
                TestRunner::<RT, S>::bank_gas_balance(&other_sequencer_address, state),
                Some(other_sequencer_balance_ref.get() - TEST_DEFAULT_USER_STAKE)
            );
        }),
    };
    let exit = TransactionTestCase {
        input: additional_sequencer.create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Exit {
                da_address: other_sequencer_da_address.as_ref().to_vec(),
            },
        ),
        assert: Box::new(move |result, state| {
            // Assert that the sequencer has correctly been unregistered
            assert!(
                TestSequencerRegistry::default()
                    .is_sender_allowed(&MockAddress::new(NON_DEFAULT_SEQUENCER_DA_ADDRESS), state)
                    .is_err(),
                "The sequencer should be registered"
            );
            // Assert that an exit event has been emitted
            assert!(result.events.iter().any(|event| matches!(
                event,
                TestRuntimeEvent::SequencerRegistry(
                    sov_sequencer_registry::Event::Exited { sequencer, amount_withdrawn }
                ) if *sequencer == other_sequencer_address && *amount_withdrawn == 100000000
            )));
            // Update the other sequencer's balance
            other_sequencer_balance_ref_1.sub(result.gas_value_used);
            // Assert that the other sequencer balance has been updated and that he recovered his bond
            assert_eq!(
                TestRunner::<RT, S>::bank_gas_balance(&other_sequencer_address, state),
                Some(other_sequencer_balance_ref_1.get())
            );
        }),
    };

    runner
        .execute_transaction(register)
        .execute_transaction(exit);
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

    runner.execute_transaction(TransactionTestCase {
        input: default_sequencer.create_plain_message::<TestSequencerRegistry>(CallMessage::Exit {
            da_address: default_sequencer.da_address.as_ref().to_vec(),
        }),
        assert: Box::new(move |result, _state| match &result.tx_receipt {
            TxEffect::Reverted(reason) => {
                assert_eq!(
                    reason.reason,
                    ModuleError(
                        TestSequencerRegistryError::Custom(
                            CustomError::CannotUnregisterDuringOwnBatch(
                                default_sequencer.da_address,
                            )
                        )
                        .into(),
                    ),
                    "Transaction reverted, but with unexpected reason"
                );
            }
            unexpected => panic!("Expected transaction to revert, but got: {:?}", unexpected),
        }),
    });
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

    let additional_sequencer_register = additional_sequencer
        .create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Register {
                da_address: NON_DEFAULT_SEQUENCER_DA_ADDRESS.to_vec(),
                amount: TEST_DEFAULT_USER_STAKE,
            },
        );
    let second_sequencer_register = second_sequencer.create_plain_message::<TestSequencerRegistry>(
        sov_sequencer_registry::CallMessage::Register {
            da_address: ANOTHER_SEQUENCER_DA_ADDRESS.to_vec(),
            amount: TEST_DEFAULT_USER_STAKE,
        },
    );
    runner.execute(additional_sequencer_register);
    runner.execute(second_sequencer_register);

    runner.execute_transaction(TransactionTestCase {
        input: additional_sequencer.create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Exit {
                da_address: ANOTHER_SEQUENCER_DA_ADDRESS.to_vec(),
            },
        ),
        assert: Box::new(move |result, _state| match &result.tx_receipt {
            TxEffect::Reverted(reason) => {
                assert_eq!(
                    reason.reason,
                    ModuleError(
                        TestSequencerRegistryError::Custom(
                            CustomError::SuppliedAddressDoesNotMatchTxSender {
                                parameter: second_sequencer.address(),
                                sender: additional_sequencer.address(),
                            },
                        )
                        .into(),
                    ),
                    "Transaction reverted, but with unexpected reason"
                );
            }
            unexpected => panic!("Expected transaction to revert, but got: {:?}", unexpected),
        }),
    });
}

/// By default the genesis sequencer is also the preferred sequencer. This checks whether the preferred sequencer is returned correctly.
#[test]
fn test_get_preferred_sequencer() {
    let (
        TestRoles {
            default_sequencer, ..
        },
        runner,
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

    let register_additional_sequencer = additional_sequencer
        .create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Register {
                da_address: ANOTHER_SEQUENCER_DA_ADDRESS.as_ref().to_vec(),
                amount: TEST_DEFAULT_USER_STAKE,
            },
        );
    let exit_default_sequencer = default_sequencer.create_plain_message::<TestSequencerRegistry>(
        sov_sequencer_registry::CallMessage::Exit {
            da_address: default_sequencer.da_address.as_ref().to_vec(),
        },
    );

    // Register the additional sequencer
    runner.execute(register_additional_sequencer);

    runner.config.sequencer_da_address = additional_sequencer_da_address.into();
    // Then exit the normal sequencer
    runner.execute(exit_default_sequencer);

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

    let default_sequencer_balance = default_sequencer.user_info.available_gas_balance;
    let default_sequencer_address = default_sequencer.user_info.address();

    runner.execute_transaction(TransactionTestCase {
        input: default_sequencer.create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Deposit {
                da_address: default_sequencer.da_address.as_ref().to_vec(),
                amount: default_sequencer_balance + TEST_DEFAULT_USER_STAKE,
            },
        ),
        assert: Box::new(move |result, _state| match &result.tx_receipt {
            TxEffect::Reverted(reason) => {
                assert_eq!(
                    reason.reason,
                    ModuleError(
                        TestSequencerRegistryError::InsufficientFundsToTopUpAccount {
                            address: default_sequencer_address,
                            amount_to_add: default_sequencer_balance + TEST_DEFAULT_USER_STAKE,
                        }
                        .into(),
                    ),
                    "Transaction reverted, but with unexpected reason"
                );
            }
            unexpected => panic!("Expected transaction to revert, but got: {:?}", unexpected),
        }),
    });
}

#[test]
fn test_non_registered_sequencer_is_not_allowed() {
    let (_, runner) = setup();

    runner.query_state(|state| {
        assert!(
            TestSequencerRegistry::default()
                .is_sender_allowed(&MockAddress::from(NON_DEFAULT_SEQUENCER_DA_ADDRESS), state)
                .is_err(),
            "Non-registered sequencers should not be allowed"
        );

        assert_eq!(
            TestSequencerRegistry::default()
                .resolve_da_address(&MockAddress::from(NON_DEFAULT_SEQUENCER_DA_ADDRESS), state)
                .unwrap_infallible(),
            None
        );
    });
}

#[test]
fn test_non_registered_sequencer_cannot_send_batches() {
    let (TestRoles { admin, .. }, mut runner) = setup();

    runner.config.sequencer_da_address = NON_DEFAULT_SEQUENCER_DA_ADDRESS.into();

    let outcome = runner.execute(BatchType(vec![admin
        .create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Register {
                da_address: NON_DEFAULT_SEQUENCER_DA_ADDRESS.to_vec(),
                amount: TEST_DEFAULT_USER_STAKE,
            },
        )]));

    assert!(outcome.batch_receipts.is_empty());
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

    runner.execute_transaction(TransactionTestCase {
        input: additional_sequencer.create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Deposit {
                da_address: NON_DEFAULT_SEQUENCER_DA_ADDRESS.as_ref().to_vec(),
                amount: TEST_DEFAULT_USER_STAKE,
            },
        ),
        assert: Box::new(move |result, _state| match &result.tx_receipt {
            TxEffect::Reverted(reason) => {
                assert_eq!(
                    reason.reason,
                    ModuleError(
                        TestSequencerRegistryError::IsNotRegistered(MockAddress::from(
                            NON_DEFAULT_SEQUENCER_DA_ADDRESS,
                        ))
                        .into()
                    ),
                    "Transaction reverted, but with unexpected reason"
                );
            }
            unexpected => panic!("Expected transaction to revert, but got: {:?}", unexpected),
        }),
    });
}
