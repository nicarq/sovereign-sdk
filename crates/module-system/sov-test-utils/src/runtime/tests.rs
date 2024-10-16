use sov_bank::{config_gas_token_id, Bank, Coins};
use sov_chain_state::ChainState;
use sov_mock_da::MockAddress;
use sov_modules_api::capabilities::FatalError;
use sov_modules_api::da::Time;
use sov_modules_api::macros::config_value;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::transaction::{PriorityFeeBips, TxDetails};
use sov_modules_api::{DaSpec, GasUnit};
use sov_modules_stf_blueprint::TxProcessingError;
use sov_sequencer_registry::SequencerRegistry;
use sov_value_setter::{ValueSetter, ValueSetterConfig};

use super::TestOptimisticRuntime;
use crate::interface::AsUser;
use crate::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use crate::runtime::{GenesisConfig, TestRunner};
use crate::{
    BatchTestCase, MockDaSpec, TestSequencer, TestUser, TransactionTestCase, TransactionType,
    TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE, TEST_DEFAULT_USER_BALANCE,
};

type S = crate::TestSpec;

/// Sets up a test runner with the [`ValueSetter`] with a single additional admin account.
fn setup() -> (TestUser<S>, TestRunner<TestOptimisticRuntime<S>, S>) {
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);

    let admin = genesis_config.additional_accounts.first().unwrap().clone();

    let value_setter_config = ValueSetterConfig {
        admin: admin.address(),
    };

    // Run genesis registering the attester and sequencer we've generated.
    let genesis = GenesisConfig::from_minimal_config(genesis_config.into(), value_setter_config);

    let runner = TestRunner::new_with_genesis(
        genesis.into_genesis_params(),
        TestOptimisticRuntime::default(),
    );

    (admin, runner)
}

#[test]
fn test_query_runtime() {
    let (admin, mut runner) = setup();

    let admin_genesis_address = runner.query_state(|state| {
        assert_eq!(
            ValueSetter::<S>::default()
                .value
                .get(state)
                .unwrap_infallible(),
            None,
            "The value should not be set"
        );

        ValueSetter::<S>::default()
            .admin
            .get(state)
            .unwrap_infallible()
            .expect("The admin should be set")
    });

    assert_eq!(
        admin.address(),
        admin_genesis_address,
        "The admins don't match"
    );

    runner.execute_transaction(TransactionTestCase {
        input: admin
            .create_plain_message::<ValueSetter<S>>(sov_value_setter::CallMessage::SetValue(1)),
        assert: Box::new(move |_result, state| {
            let value = ValueSetter::<S>::default()
                .value
                .get(state)
                .unwrap_infallible();
            assert_eq!(value, Some(1), "The value should be set to 1");
        }),
    });
}

/// Tests that the batch is rewarded if the default sequencer is used
#[test]
fn test_default_sequencer() {
    let (admin, mut runner) = setup();

    runner.execute_batch(BatchTestCase {
        input: vec![admin
            .create_plain_message::<ValueSetter<S>>(sov_value_setter::CallMessage::SetValue(1))]
        .into(),
        assert: Box::new(move |result, _state| {
            assert_eq!(result.sender_da_address, runner.config.sequencer_da_address);
        }),
    });
}

/// Tests that the batch is dropped if the specified sequencer is not registered
#[test]
fn test_specify_non_default_sequencer_errors_if_not_registered() {
    let (admin, mut runner) = setup();

    runner.config.sequencer_da_address = <MockDaSpec as DaSpec>::Address::from([42; 32]);

    runner.execute_batch(BatchTestCase {
        input: vec![admin
            .create_plain_message::<ValueSetter<S>>(sov_value_setter::CallMessage::SetValue(10))]
        .into(),
        assert: Box::new(move |result, _state| {
            assert!(
                result.batch_receipt.is_none(),
                "Batch should have been dropped"
            );
        }),
    });
}

/// Tests that we can register and use another sequencer
#[test]
fn test_register_sequencer() {
    let (additional_user, mut runner) = setup();

    let new_sequencer_address = MockAddress::from([42; 32]);

    let user_stake_value = runner.query_state(|state| {
        SequencerRegistry::<S>::default()
            .get_coins_to_lock(state)
            .unwrap_infallible()
            .amount
    });

    let new_sequencer = TestSequencer::<S> {
        user_info: additional_user,
        da_address: new_sequencer_address,
        bond: user_stake_value,
    };

    // We first bond the sequencer
    runner.execute(new_sequencer.create_plain_message::<SequencerRegistry<S>>(
        sov_sequencer_registry::CallMessage::Register {
            da_address: new_sequencer.da_address,
            amount: new_sequencer.bond,
        },
    ));

    runner.config.sequencer_da_address = new_sequencer.da_address;

    runner
        // Then we use the non-default sequencer to set a value
        .execute_batch(BatchTestCase {
            input: vec![new_sequencer.create_plain_message::<ValueSetter<S>>(
                sov_value_setter::CallMessage::SetValue(10),
            )]
            .into(),
            assert: Box::new(move |result, state| {
                assert_eq!(result.sender_da_address, new_sequencer_address);
                // ensure the tx was applied / batch was accepted
                assert_eq!(
                    sov_value_setter::ValueSetter::<S>::default()
                        .value
                        .get(state)
                        .unwrap_infallible(),
                    Some(10)
                );
            }),
        });
}

/// Checks that the chain id of a transaction can be overridden.
#[test]
fn test_custom_transaction_details_chain_id() {
    let (admin, mut runner) = setup();

    let real_chain_id = config_value!("CHAIN_ID");
    let fake_chain_id = real_chain_id + 1;

    runner.execute_batch(BatchTestCase {
        input: vec![admin
            .create_plain_message::<ValueSetter<S>>(sov_value_setter::CallMessage::SetValue(1))
            .with_chain_id(fake_chain_id)]
        .into(),
        assert: Box::new(move |result, _state| {
            match result.batch_receipt.unwrap().inner.outcome {
                sov_modules_api::BatchSequencerOutcome::Ignored(reason) => {
                    assert_eq!(
                        reason,
                        FatalError::InvalidChainId {
                            expected: real_chain_id,
                            got: fake_chain_id
                        }
                        .to_string()
                    );
                }
                unexpected => panic!("Expected batch slashed, but got {:?}", unexpected),
            };
        }),
    });
}

/// Checks that the max fee of a transaction can be overridden.
#[test]
fn test_custom_transaction_details_max_fee() {
    let (admin, mut runner) = setup();

    runner.execute_transaction(TransactionTestCase {
        input: admin
            .create_plain_message::<ValueSetter<S>>(sov_value_setter::CallMessage::SetValue(10))
            .with_max_fee(0),
        assert: Box::new(move |result, _state| {
           match &result.tx_receipt {
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
        }),
    });
}

/// Checks that the priority fee of a transaction can be overridden and that this has the expected effect on the balance of the sender.
#[test]
fn test_custom_transaction_details_priority_fee_bips() {
    let (admin, mut runner) = setup();

    let max_fee = admin.available_gas_balance;
    let priority_fee_bips = PriorityFeeBips::from_percentage(5);

    runner.execute_transaction(TransactionTestCase {
        input: admin
            .create_plain_message::<ValueSetter<S>>(sov_value_setter::CallMessage::SetValue(10))
            .with_max_fee(max_fee)
            .with_max_priority_fee_bips(priority_fee_bips),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());

            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&admin.address(), config_gas_token_id(), state)
                    .unwrap_infallible(),
                Some(admin.available_gas_balance - result.gas_value_used - priority_fee_bips.apply(result.gas_value_used).unwrap()),
                "The admin's balance should be equal to the initial balance minus the gas used to send the transaction and the priority fee"
            );

        }),
    });
}

/// Checks that the chain id of a transaction can be overridden.
#[test]
fn test_custom_transaction_details_gas_limit() {
    let (admin, mut runner) = setup();

    runner.execute_transaction(TransactionTestCase {
        input: admin
            .create_plain_message::<ValueSetter<S>>(sov_value_setter::CallMessage::SetValue(10))
            .with_max_fee(admin.available_gas_balance)
            .with_gas_limit(Some(GasUnit::from([admin.available_gas_balance; 2]))),
        assert: Box::new(move |result, _state| {
           match &result.tx_receipt {
                sov_modules_api::TxEffect::Skipped(skipped) => {
                    if let TxProcessingError::CannotReserveGas(error_message) = &skipped.error {
                        assert!(
                            error_message.contains("The current gas price is too high to cover the maximum fee for the transaction"),
                            "Error message doesn't contain with the expected phrase. Got: {}",
                            error_message
                        );
                    } else {
                        panic!("Expected CannotReserveGas error, but got a different SkippedReason: {:?}", skipped.error);
                    }
                },
                unexpected => panic!("Expected transaction to revert, but got: {:?}", unexpected),
            };
        }),
    });
}

/// Tests that sending a transaction with the default details works and that the balance of the sender is updated correctly.
#[test]
fn test_default_transaction_details_works() {
    let (admin, mut runner) = setup();

    runner.execute_transaction(TransactionTestCase {
        input: admin
            .create_plain_message::<ValueSetter<S>>(sov_value_setter::CallMessage::SetValue(10)),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());

            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&admin.address(), config_gas_token_id(), state)
                    .unwrap_infallible(),
                Some(admin.available_gas_balance - result.gas_value_used),
                "The admin's balance should be equal to the initial balance minus the gas used to send the transaction"
            );
        }),
    });
}

/// Checks the default transaction details format.
#[test]
fn test_default_transaction_details() {
    let user = TestUser::<S>::generate(TEST_DEFAULT_USER_BALANCE);
    let message = user.create_plain_message::<Bank<S>>(sov_bank::CallMessage::Transfer {
        to: user.address(),
        coins: Coins {
            amount: 1000,
            token_id: config_gas_token_id(),
        },
    });

    match message {
        TransactionType::Plain {
            details,
            message,
            key,
        } => {
            assert_eq!(
                message,
                sov_bank::CallMessage::Transfer {
                    to: user.address(),
                    coins: Coins {
                        amount: 1000,
                        token_id: config_gas_token_id(),
                    },
                }
            );

            assert_eq!(key.as_hex(), user.private_key().as_hex());

            assert_eq!(details.max_priority_fee_bips, TEST_DEFAULT_MAX_PRIORITY_FEE);
            assert_eq!(details.max_fee, TEST_DEFAULT_MAX_FEE);
            assert_eq!(details.gas_limit, None);

            assert_eq!(details.chain_id, 4321);
        }
        _ => panic!("The message is not a plain message"),
    }
}

/// Tests that the transaction is correctly formatted
#[test]
fn test_custom_transaction_format() {
    let user = TestUser::<S>::generate(TEST_DEFAULT_USER_BALANCE);
    let message = user
        .create_plain_message::<Bank<S>>(sov_bank::CallMessage::Transfer {
            to: user.address(),
            coins: Coins {
                amount: 1000,
                token_id: config_gas_token_id(),
            },
        })
        .with_max_fee(100)
        .with_max_priority_fee_bips(PriorityFeeBips::from_percentage(10))
        .with_gas_limit(Some(GasUnit::from([5; 2])))
        .with_chain_id(5555);

    match message {
        TransactionType::Plain {
            details,
            message,
            key,
        } => {
            assert_eq!(
                message,
                sov_bank::CallMessage::Transfer {
                    to: user.address(),
                    coins: Coins {
                        amount: 1000,
                        token_id: config_gas_token_id(),
                    },
                }
            );

            assert_eq!(key.as_hex(), user.private_key().as_hex());

            assert_eq!(
                details.max_priority_fee_bips,
                PriorityFeeBips::from_percentage(10)
            );
            assert_eq!(details.max_fee, 100);
            assert_eq!(details.gas_limit, Some(GasUnit::from([5; 2])));

            assert_eq!(details.chain_id, 5555);
        }
        _ => panic!("The message is not a plain message"),
    }
}

#[test]
fn test_custom_transaction_format_2() {
    let user = TestUser::<S>::generate(TEST_DEFAULT_USER_BALANCE);
    let message = user
        .create_plain_message::<Bank<S>>(sov_bank::CallMessage::Transfer {
            to: user.address(),
            coins: Coins {
                amount: 1000,
                token_id: config_gas_token_id(),
            },
        })
        .with_details(TxDetails {
            max_fee: 100,
            max_priority_fee_bips: PriorityFeeBips::from_percentage(10),
            gas_limit: Some(GasUnit::from([5; 2])),
            chain_id: 5555,
        });

    match message {
        TransactionType::Plain {
            details,
            message,
            key,
        } => {
            assert_eq!(
                message,
                sov_bank::CallMessage::Transfer {
                    to: user.address(),
                    coins: Coins {
                        amount: 1000,
                        token_id: config_gas_token_id(),
                    },
                }
            );

            assert_eq!(key.as_hex(), user.private_key().as_hex());

            assert_eq!(
                details.max_priority_fee_bips,
                PriorityFeeBips::from_percentage(10)
            );
            assert_eq!(details.max_fee, 100);
            assert_eq!(details.gas_limit, Some(GasUnit::from([5; 2])));

            assert_eq!(details.chain_id, 5555);
        }
        _ => panic!("The message is not a plain message"),
    }
}

#[test]
fn test_freeze_time() {
    let (_, mut runner) = setup();
    let chain_state = ChainState::<S>::default();

    runner.config.freeze_time = Some(Time::from_secs(200));
    let time = runner.query_kernel_state(|state| chain_state.get_time(state).unwrap_infallible());
    // not frozen until the next slot.
    assert_ne!(time, Time::from_secs(200));

    runner.advance_slots(1);

    let time = runner.query_kernel_state(|state| chain_state.get_time(state).unwrap_infallible());
    assert_eq!(time, Time::from_secs(200));

    runner.advance_slots(1);
    let time = runner.query_kernel_state(|state| chain_state.get_time(state).unwrap_infallible());
    // time is still frozen
    assert_eq!(time, Time::from_secs(200));

    runner.config.freeze_time = Some(Time::from_secs(5000));
    // time is still frozen in the current slot, as it is not advanced.
    assert_eq!(time, Time::from_secs(200));

    runner.advance_slots(1);

    let time = runner.query_kernel_state(|state| chain_state.get_time(state).unwrap_infallible());
    // frozen time is updated
    assert_eq!(time, Time::from_secs(5000));

    // timestamps should revert to the current time
    runner.config.freeze_time = None;
    let time = runner.query_kernel_state(|state| chain_state.get_time(state).unwrap_infallible());
    // frozen time is still frozen until next slot
    assert_eq!(time, Time::from_secs(5000));

    runner.advance_slots(1);

    let time = runner.query_kernel_state(|state| chain_state.get_time(state).unwrap_infallible());

    assert!(
        Time::from_secs(5000) < time,
        "Time should no longer be frozen"
    );
}
