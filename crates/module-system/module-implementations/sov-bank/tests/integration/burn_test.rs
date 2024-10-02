use sov_bank::event::Event;
use sov_bank::utils::TokenHolder;
use sov_bank::{config_gas_token_id, Bank, Coins};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{Error, TxEffect};
use sov_test_utils::runtime::genesis::TestTokenName;
use sov_test_utils::{AsUser, TransactionTestCase};

use crate::helpers::{setup, TestBankRuntimeEvent, TestData};

type S = sov_test_utils::TestSpec;

/// We can burn tokens that are deployed on the bank.
#[test]
fn burn_deployed_tokens_happy_path() {
    let (
        TestData {
            token_name,
            token_id,
            user_high_token_balance,
            ..
        },
        mut runner,
    ) = setup();

    let user_token_balance = user_high_token_balance.token_balance(&token_name).unwrap();

    let user_address = user_high_token_balance.address();

    runner.execute_transaction(TransactionTestCase {
        input: user_high_token_balance.create_plain_message::<sov_bank::Bank<S>>(
            sov_bank::CallMessage::Burn {
                coins: Coins {
                    amount: user_token_balance,
                    token_id,
                },
            },
        ),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            assert_eq!(result.events.len(), 1);
            assert_eq!(
                TestBankRuntimeEvent::Bank(Event::TokenBurned {
                    owner: TokenHolder::User(user_address),
                    coins: Coins {
                        amount: user_token_balance,
                        token_id
                    }
                }),
                result.events[0]
            );

            // Check that the user's balance is now zero
            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&user_address, token_id, state)
                    .unwrap_infallible(),
                Some(0),
                "The user's balance should be zero"
            );
        }),
    });
}

#[test]
fn burn_deployed_tokens_no_balance_fails() {
    let (
        TestData {
            token_id,
            user_no_token_balance,
            ..
        },
        mut runner,
    ) = setup();

    let user_address = user_no_token_balance.address();
    const BURN_AMOUNT: u64 = 1;

    let initial_total_supply = runner.query_state(|state| {
        Bank::<S>::default()
            .get_total_supply_of(&token_id, state)
            .unwrap_infallible()
            .unwrap()
    });

    runner.execute_transaction(TransactionTestCase {
        input: user_no_token_balance.create_plain_message::<sov_bank::Bank<S>>(
            sov_bank::CallMessage::Burn {
                coins: Coins {
                    amount: BURN_AMOUNT,
                    token_id,
                },
            },
        ),
        assert: Box::new(move |result, state| {
            assert!(
                matches!(result.tx_receipt, TxEffect::Reverted(..)),
                "The transaction should have been reverted"
            );

            // Burn by another user, who doesn't have tokens at all
            match result.tx_receipt {
                TxEffect::Reverted(contents) => {
                    let Error::ModuleError(err) = contents.reason;
                    let mut chain = err.chain();
                    let message_1 = chain.next().unwrap().to_string();
                    let message_2 = chain.next().unwrap().to_string();
                    assert!(chain.next().is_none());
                    assert_eq!(
                        format!(
                            "Failed to burn coins(token_id={} amount={}) from owner {}",
                            token_id, BURN_AMOUNT, user_address
                        ),
                        message_1
                    );
                    let expected_error_part = format!(
                        "Value not found for prefix: \"sov_bank/Bank/tokens/{}\" and storage key:",
                        token_id
                    );
                    assert!(message_2.starts_with(&expected_error_part));
                }
                _ => {
                    panic!("The transaction should have been reverted")
                }
            }

            let final_total_supply = Bank::<S>::default()
                .get_total_supply_of(&token_id, state)
                .unwrap_infallible()
                .unwrap();

            assert_eq!(
                initial_total_supply, final_total_supply,
                "The token supply shouldn't have changed"
            );
        }),
    });
}

#[test]
fn burn_more_than_deployed_tokens_fails() {
    let (
        TestData {
            token_id,
            token_name,
            user_high_token_balance,
            ..
        },
        mut runner,
    ) = setup();

    let total_token_supply = runner.query_state(|state| {
        Bank::<S>::default()
            .get_total_supply_of(&token_id, state)
            .unwrap_infallible()
            .unwrap()
    });

    let to_burn = total_token_supply + 1;

    let user_address = user_high_token_balance.address();
    let user_token_balance = user_high_token_balance.token_balance(&token_name).unwrap();

    runner.execute_transaction(TransactionTestCase {
        input: user_high_token_balance.create_plain_message::<sov_bank::Bank<S>>(
            sov_bank::CallMessage::Burn {
                coins: Coins {
                    amount: to_burn,
                    token_id,
                },
            },
        ),
        assert: Box::new(move |result, state| match result.tx_receipt {
            TxEffect::Reverted(contents) => {
                let Error::ModuleError(err) = contents.reason;
                let mut chain = err.chain();

                let message_1 = chain.next().unwrap().to_string();
                let message_2 = chain.next().unwrap().to_string();

                assert!(chain.next().is_none());
                assert_eq!(
                    message_1,
                    format!(
                        "Failed to burn coins(token_id={} amount={}) from owner {}",
                        token_id, to_burn, user_address
                    )
                );

                assert_eq!(
                    format!(
                        "Insufficient balance from={user_address}, got={}, needed={}, for token={}",
                        user_token_balance, to_burn, token_name
                    ),
                    message_2,
                    "The error message is incorrect"
                );

                let final_total_supply = Bank::<S>::default()
                    .get_total_supply_of(&token_id, state)
                    .unwrap_infallible()
                    .unwrap();

                assert_eq!(
                    total_token_supply, final_total_supply,
                    "The token supply shouldn't have changed"
                );
            }
            _ => panic!("The outcome is incorrect"),
        }),
    });
}

#[test]
fn burn_more_than_available_balance_fails() {
    let (
        TestData {
            token_id,
            token_name,
            user_high_token_balance,
            ..
        },
        mut runner,
    ) = setup();

    let initial_token_supply = runner.query_state(|state| {
        Bank::<S>::default()
            .get_total_supply_of(&token_id, state)
            .unwrap_infallible()
            .unwrap()
    });

    let user_token_balance = user_high_token_balance.token_balance(&token_name).unwrap();

    let user_address = user_high_token_balance.address();

    let to_burn = user_token_balance + 1;

    runner.execute_transaction(TransactionTestCase {
        input: user_high_token_balance.create_plain_message::<sov_bank::Bank<S>>(
            sov_bank::CallMessage::Burn {
                coins: Coins {
                    amount: to_burn,
                    token_id,
                },
            },
        ),
        assert: Box::new(move |result, state| match result.tx_receipt {
            TxEffect::Reverted(contents) => {
                let Error::ModuleError(err) = contents.reason;
                let mut chain = err.chain();

                let message_1 = chain.next().unwrap().to_string();
                let message_2 = chain.next().unwrap().to_string();

                assert!(chain.next().is_none());
                assert_eq!(
                    message_1,
                    format!(
                        "Failed to burn coins(token_id={} amount={}) from owner {}",
                        token_id, to_burn, user_address
                    )
                );

                assert_eq!(
                    format!(
                        "Insufficient balance from={user_address}, got={}, needed={}, for token={}",
                        user_token_balance, to_burn, token_name
                    ),
                    message_2,
                    "The error message is incorrect"
                );

                let final_total_supply = Bank::<S>::default()
                    .get_total_supply_of(&token_id, state)
                    .unwrap_infallible()
                    .unwrap();

                assert_eq!(
                    initial_token_supply, final_total_supply,
                    "The token supply shouldn't have changed"
                );
            }
            _ => {
                panic!("The transaction does not have the expected outcome.")
            }
        }),
    });
}

#[test]
fn burn_deployed_tokens_zero_amount_works_if_user_has_tokens() {
    let (
        TestData {
            token_name,
            token_id,
            user_high_token_balance,
            ..
        },
        mut runner,
    ) = setup();

    let user_token_balance = user_high_token_balance.token_balance(&token_name).unwrap();

    let user_address = user_high_token_balance.address();

    runner.execute_transaction(TransactionTestCase {
        input: user_high_token_balance.create_plain_message::<sov_bank::Bank<S>>(
            sov_bank::CallMessage::Burn {
                coins: Coins {
                    amount: 0,
                    token_id,
                },
            },
        ),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            assert_eq!(result.events.len(), 1);
            assert_eq!(
                TestBankRuntimeEvent::Bank(Event::TokenBurned {
                    owner: TokenHolder::User(user_address),
                    coins: Coins {
                        amount: 0,
                        token_id
                    }
                }),
                result.events[0]
            );

            // Check that the user's balance hasn't changed
            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&user_address, token_id, state)
                    .unwrap_infallible(),
                Some(user_token_balance),
                "The user's balance shouldn't have changed"
            );
        }),
    });
}

#[test]
fn burn_deployed_tokens_zero_amount_doesnt_work_if_user_has_no_tokens() {
    let (
        TestData {
            token_id,
            user_no_token_balance,
            ..
        },
        mut runner,
    ) = setup();

    let user_address = user_no_token_balance.address();

    runner.execute_transaction(TransactionTestCase {
        input: user_no_token_balance.create_plain_message::<sov_bank::Bank<S>>(
            sov_bank::CallMessage::Burn {
                coins: Coins {
                    amount: 0,
                    token_id,
                },
            },
        ),
        assert: Box::new(move |result, _| match result.tx_receipt {
            TxEffect::Reverted(contents) => {
                let Error::ModuleError(err) = contents.reason;
                let mut chain = err.chain();

                let message_1 = chain.next().unwrap().to_string();
                let message_2 = chain.next().unwrap().to_string();

                assert!(chain.next().is_none());
                assert_eq!(
                    message_1,
                    format!(
                        "Failed to burn coins(token_id={} amount={}) from owner {}",
                        token_id, 0, user_address
                    )
                );

                // Note, no token balance cause the message.
                let expected_error_part =
                    &format!("Value not found for prefix: \"sov_bank/Bank/tokens/{token_id}\" and storage key:");
                assert!(message_2.starts_with(expected_error_part));
            }
            _ => {
                panic!("The transaction does not have the expected outcome.")
            }
        }),
    });
}

#[test]
fn burn_unknown_token_fails() {
    let (
        TestData {
            user_high_token_balance,
            ..
        },
        mut runner,
    ) = setup();

    const AMOUNT_TO_BURN: u64 = 0;

    let other_token_id = TestTokenName::new("OtherToken".to_string()).id();

    let user_address = user_high_token_balance.address();

    runner.execute_transaction(TransactionTestCase {
        input: user_high_token_balance.create_plain_message::<sov_bank::Bank<S>>(
            sov_bank::CallMessage::Burn {
                coins: Coins {
                    amount: AMOUNT_TO_BURN,
                    token_id: other_token_id,
                },
            },
        ),
        assert: Box::new(move |result, _| {
            assert!(
                matches!(result.tx_receipt, TxEffect::Reverted(..)),
                "The transaction should have been reverted"
            );

            match result.tx_receipt {
                TxEffect::Reverted(contents) => {
                    let Error::ModuleError(err) = contents.reason;
                    let mut chain = err.chain();

                    let message_1 = chain.next().unwrap().to_string();
                    let message_2 = chain.next().unwrap().to_string();

                    assert!(chain.next().is_none());

                    assert_eq!(
                        format!(
                            "Failed to burn coins(token_id={} amount={}) from owner {}",
                            other_token_id, AMOUNT_TO_BURN, user_address
                        ),
                        message_1,
                        "The first message is incorrect"
                    );

                    // Note, no token ID in root cause the message.
                    let expected_error_part =
                        "Value not found for prefix: \"sov_bank/Bank/tokens/\" and storage key:";
                    assert!(message_2.starts_with(expected_error_part));
                }
                _ => {
                    panic!("The transaction does not have the expected outcome.")
                }
            }
        }),
    });
}

/// Simple test to check that burning the gas token works.
#[test]
fn burn_gas_token_also_works() {
    let (
        TestData {
            user_high_token_balance,
            ..
        },
        mut runner,
    ) = setup();

    let user_gas_balance = user_high_token_balance.available_gas_balance;
    let user_address = user_high_token_balance.address();

    runner.execute_transaction(TransactionTestCase {
        input: user_high_token_balance.create_plain_message::<sov_bank::Bank<S>>(
            sov_bank::CallMessage::Burn {
                coins: Coins {
                    // Note: we are only burning half of the gas balance because some of it is
                    // already consumed (for pre-execution checks) by the time we are reaching the burn method of the `Bank` module.
                    amount: user_gas_balance / 2,
                    token_id: config_gas_token_id(),
                },
            },
        ),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            assert_eq!(result.events.len(), 1);
            assert_eq!(
                TestBankRuntimeEvent::Bank(Event::TokenBurned {
                    owner: TokenHolder::User(user_address),
                    coins: Coins {
                        amount: user_gas_balance / 2,
                        token_id: config_gas_token_id()
                    }
                }),
                result.events[0]
            );

            // Check that the user's gas balance is now equal to the burnt amount minus the gas used to send the transaction
            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&user_address, config_gas_token_id(), state)
                    .unwrap_infallible(),
                Some(user_gas_balance / 2 - result.gas_value_used),
                "The user's balance should be zero"
            );
        }),
    });
}
