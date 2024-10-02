use std::collections::HashMap;

use sov_bank::{config_gas_token_id, Bank};
use sov_mock_da::MockAddress;
use sov_modules_api::macros::config_value;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::transaction::PriorityFeeBips;
use sov_modules_api::Error::ModuleError;
use sov_modules_api::{Gas, GasArray, GasMeter, Spec, TxEffect};
use sov_sequencer_registry::{CallMessage, CustomError, SequencerRegistry};
use sov_test_utils::runtime::{TestRunner, ValueSetter};
use sov_test_utils::{
    AsUser, AtomicNumber, BatchTestCase, BatchType, TransactionTestCase, TransactionType,
    TEST_DEFAULT_USER_BALANCE,
};

use crate::helpers::{
    minimal_bond, setup, setup_with_custom_runtime, Da, TestRoles, TestRuntimeEvent,
    TestSequencerRegistry, TestSequencerRegistryError, ANOTHER_SEQUENCER_DA_ADDRESS,
    NON_DEFAULT_SEQUENCER_DA_ADDRESS, RT,
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

    let user_stake_value = minimal_bond(&runner);

    runner.execute_transaction(TransactionTestCase {
        input: additional_sequencer.create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Register {
                da_address: other_sequencer_da_address.as_ref().to_vec(),
                amount: user_stake_value,
            },
        ),
        assert: Box::new(move |result, state| {
            // Assert that the sequencer has correctly been registered
            assert!(
                TestSequencerRegistry::default()
                    .is_sender_allowed(
                        &MockAddress::new(NON_DEFAULT_SEQUENCER_DA_ADDRESS),
                        &state.gas_price().clone(),
                        state
                    )
                    .is_ok(),
                "The sequencer is not registered"
            );
            // Assert that a registration event has been emitted
            assert!(result.events.iter().any(|event| matches!(
                event,
                TestRuntimeEvent::SequencerRegistry(
                    sov_sequencer_registry::Event::Registered { sequencer, amount }
                ) if *sequencer == other_sequencer_address && *amount == user_stake_value
            )));
            // Assert that the other sequencer balance has been updated
            assert_eq!(
                TestRunner::<RT, S>::bank_gas_balance(&other_sequencer_address, state),
                Some(TEST_DEFAULT_USER_BALANCE - user_stake_value - result.gas_value_used)
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

    let user_stake_value = minimal_bond(&runner);

    let amount_to_register = other_sequencer_balance + user_stake_value;

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

    let user_stake_value = minimal_bond(&runner);

    runner.execute(
        additional_sequencer.create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Register {
                da_address: NON_DEFAULT_SEQUENCER_DA_ADDRESS.to_vec(),
                amount: user_stake_value,
            },
        ),
    );

    runner.execute_transaction(TransactionTestCase {
        input: additional_sequencer.create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Register {
                da_address: NON_DEFAULT_SEQUENCER_DA_ADDRESS.to_vec(),
                amount: user_stake_value,
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

    let user_stake_value = minimal_bond(&runner);

    let register = TransactionTestCase {
        input: additional_sequencer.create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Register {
                da_address: other_sequencer_da_address.as_ref().to_vec(),
                amount: user_stake_value,
            },
        ),
        assert: Box::new(move |result, state| {
            // Assert that the sequencer has correctly been registered
            assert!(
                TestSequencerRegistry::default()
                    .is_sender_allowed(
                        &MockAddress::new(NON_DEFAULT_SEQUENCER_DA_ADDRESS),
                        &state.gas_price().clone(),
                        state
                    )
                    .is_ok(),
                "The sequencer is not registered"
            );
            // Update the other sequencer's balance
            other_sequencer_balance_ref.sub(result.gas_value_used);
            // Assert that the other sequencer balance has been updated
            assert_eq!(
                TestRunner::<RT, S>::bank_gas_balance(&other_sequencer_address, state),
                Some(other_sequencer_balance_ref.get() - user_stake_value)
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
                    .is_sender_allowed(
                        &MockAddress::new(NON_DEFAULT_SEQUENCER_DA_ADDRESS),
                        &state.gas_price().clone(),
                        state
                    )
                    .is_err(),
                "The sequencer should be registered"
            );
            let expected_balance_to_withdraw = user_stake_value;
            // Assert that an exit event has been emitted
            assert!(result.events.iter().any(|event| matches!(
                event,
                TestRuntimeEvent::SequencerRegistry(
                    sov_sequencer_registry::Event::Exited { sequencer, amount_withdrawn }
                ) if *sequencer == other_sequencer_address && *amount_withdrawn == expected_balance_to_withdraw
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

    let user_stake_value = minimal_bond(&runner);

    let additional_sequencer_register = additional_sequencer
        .create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Register {
                da_address: NON_DEFAULT_SEQUENCER_DA_ADDRESS.to_vec(),
                amount: user_stake_value,
            },
        );
    let second_sequencer_register = second_sequencer.create_plain_message::<TestSequencerRegistry>(
        sov_sequencer_registry::CallMessage::Register {
            da_address: ANOTHER_SEQUENCER_DA_ADDRESS.to_vec(),
            amount: user_stake_value,
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
    let user_stake_value = minimal_bond(&runner);

    let register_additional_sequencer = additional_sequencer
        .create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Register {
                da_address: ANOTHER_SEQUENCER_DA_ADDRESS.as_ref().to_vec(),
                amount: user_stake_value,
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
    let user_stake_value = minimal_bond(&runner);

    runner.execute_transaction(TransactionTestCase {
        input: default_sequencer.create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Deposit {
                da_address: default_sequencer.da_address.as_ref().to_vec(),
                amount: default_sequencer_balance + user_stake_value,
            },
        ),
        assert: Box::new(move |result, _state| match &result.tx_receipt {
            TxEffect::Reverted(reason) => {
                assert_eq!(
                    reason.reason,
                    ModuleError(
                        TestSequencerRegistryError::InsufficientFundsToTopUpAccount {
                            address: default_sequencer_address,
                            amount_to_add: default_sequencer_balance + user_stake_value,
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
                .is_sender_allowed(
                    &MockAddress::from(NON_DEFAULT_SEQUENCER_DA_ADDRESS),
                    &state.gas_price().clone(),
                    state
                )
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
    let user_stake_value = minimal_bond(&runner);

    let outcome = runner.execute(BatchType(vec![admin
        .create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Register {
                da_address: NON_DEFAULT_SEQUENCER_DA_ADDRESS.to_vec(),
                amount: user_stake_value,
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

    let user_stake_value = minimal_bond(&runner);

    runner.execute_transaction(TransactionTestCase {
        input: additional_sequencer.create_plain_message::<TestSequencerRegistry>(
            sov_sequencer_registry::CallMessage::Deposit {
                da_address: NON_DEFAULT_SEQUENCER_DA_ADDRESS.as_ref().to_vec(),
                amount: user_stake_value,
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

/// This test ensures that the sequencer cannot sequence anymore when the gas price is too high.
/// That is, if the gas price increases, the sequencer will not have bonded enough funds and then won't be able to sequence anymore.
/// Currently, the easiest way to do this is to artificially change the gas cost of some operation in the bank module. We do that
/// by modifying the runtime manually.
#[test]
fn test_cannot_sequence_when_gas_price_is_too_high() {
    let mut gas_limit = <S as Spec>::Gas::from(config_value!("INITIAL_GAS_LIMIT"));
    let gas_target = gas_limit.scalar_division(2).clone();

    let zero_gas = <S as Spec>::Gas::zero();

    let mut runtime = RT::default();

    runtime
        .bank
        .override_gas_config(sov_bank::BankGasConfig::<<S as Spec>::Gas> {
            burn: gas_target.clone(),
            mint: zero_gas.clone(),
            create_token: zero_gas.clone(),
            transfer: zero_gas.clone(),
            freeze: zero_gas.clone(),
        });

    let (roles, mut runner) = setup_with_custom_runtime(runtime);

    let mut nonces = HashMap::new();

    let additional_sequencer_da_address = MockAddress::new([1; 32]);

    let additional_sequencer_bond = minimal_bond(&runner);

    let initial_gas_price = runner.query_state(|state| state.gas_price().clone());

    let (bank_signed, register_signed) = runner.query_state(|state| {
        let bank_signed = roles
            .admin
            .create_plain_message::<Bank<S>>(sov_bank::CallMessage::Burn {
                coins: sov_bank::Coins {
                    amount: 1,
                    token_id: config_gas_token_id(),
                },
            })
            .with_max_fee(roles.admin.available_gas_balance / 2)
            .to_serialized_authenticated_tx::<RT>(&mut nonces, state);

        let register_signed = roles
            .additional_sequencer
            .create_plain_message::<SequencerRegistry<S, Da>>(CallMessage::Register {
                da_address: additional_sequencer_da_address.as_ref().to_vec(),
                amount: additional_sequencer_bond,
            })
            .to_serialized_authenticated_tx::<RT>(&mut nonces, state);

        (bank_signed, register_signed)
    });

    // We execute a batch of two transactions, check that the total gas used is higher than the target.
    runner.execute_batch(BatchTestCase {
        input: BatchType(vec![
            TransactionType::<SequencerRegistry<S, Da>, S>::PreAuthenticated(bank_signed),
            TransactionType::<SequencerRegistry<S, Da>, S>::PreAuthenticated(register_signed),
        ]),
        assert: Box::new(move |result, _state| {
            assert_eq!(result.batch_receipt.clone().unwrap().tx_receipts.len(), 2);

            let mut total_gas_used = <S as Spec>::Gas::zero();

            for (i, tx_receipt) in result.batch_receipt.unwrap().tx_receipts.iter().enumerate() {
                match &tx_receipt.receipt {
                    TxEffect::Successful(tx_contents) => {
                        total_gas_used.combine(&tx_contents.gas_used);
                    }
                    _ => {
                        panic!("Tx {i} with receipt {tx_receipt:?} should be successful");
                    }
                }
            }

            assert!(
                total_gas_used > gas_target,
                "The total gas used should be higher than the initial gas used"
            );
        }),
    });

    // We advance one slot to reflect the gas update on the state.
    runner.advance_slots(1);

    let new_bond_amount = minimal_bond(&runner);

    runner.query_state(|state| {
        let new_gas_price = state.gas_price().clone();

        assert!(
            new_gas_price > initial_gas_price,
            "The new gas price {new_gas_price} should be higher than the initial gas price {initial_gas_price}"
        );

        assert!(
            new_bond_amount > additional_sequencer_bond,
            "The new bond amount {new_bond_amount} should be higher than the initial additional sequencer bond {additional_sequencer_bond}."
        );

        // The sequencer should be registered
        assert!(
            SequencerRegistry::<S, Da>::default()
                .is_registered_sequencer(&additional_sequencer_da_address, state)
                .unwrap_infallible(),
            "The additional sequencer should be registered"
        );

        // But he should not be allowed to send transactions because he doesn't have enough stake.
        assert_eq!(
            SequencerRegistry::<S, Da>::default().is_sender_allowed(
                &additional_sequencer_da_address,
                &new_gas_price,
                state
            ),
            Err(
                sov_sequencer_registry::AllowedSequencerError::InsufficientStakeAmount {
                    bond_amount: additional_sequencer_bond,
                    minimum_bond_amount: new_bond_amount
                }
            )
        );
    });
}
