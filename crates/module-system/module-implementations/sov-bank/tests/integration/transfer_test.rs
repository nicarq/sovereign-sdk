use sov_bank::utils::TokenHolder;
use sov_bank::{config_gas_token_id, Bank, CallMessage, Coins};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{Error, TxEffect};
use sov_test_utils::runtime::genesis::TestTokenName;
use sov_test_utils::{AsUser, TestUser, TransactionTestCase};

use crate::helpers::*;

type S = sov_test_utils::TestSpec;

/// Tests the happy path of a transfer call. Transfer a given amount of tokens from a user with a high balance to another user.
#[test]
fn transfer_token_happy_path() {
    let (
        TestData {
            token_id,
            token_name,
            user_high_token_balance,
            user_no_token_balance,
            ..
        },
        mut runner,
    ) = setup();

    const TRANSFER_AMOUNT: u64 = 10;
    let user_high_token_balance_address = user_high_token_balance.address();
    let user_high_token_initial_balance =
        user_high_token_balance.token_balance(&token_name).unwrap();

    let user_no_token_balance_address = user_no_token_balance.address();

    runner.execute_transaction(TransactionTestCase {
        input: user_high_token_balance.create_plain_message::<RT, Bank<S>>(CallMessage::Transfer {
            to: user_no_token_balance_address,
            coins: Coins {
                amount: TRANSFER_AMOUNT,
                token_id,
            },
        }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            assert_eq!(result.events.len(), 1);
            assert_eq!(
                result.events[0],
                TestBankRuntimeEvent::Bank(sov_bank::event::Event::TokenTransferred {
                    from: TokenHolder::User(user_high_token_balance_address),
                    to: TokenHolder::User(user_no_token_balance_address),
                    coins: Coins {
                        amount: TRANSFER_AMOUNT,
                        token_id
                    }
                })
            );

            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&user_no_token_balance_address, token_id, state)
                    .unwrap_infallible(),
                Some(TRANSFER_AMOUNT)
            );

            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&user_high_token_balance_address, token_id, state)
                    .unwrap_infallible(),
                Some(user_high_token_initial_balance - TRANSFER_AMOUNT)
            );
        }),
    });
}

/// Tests that a transfer call fails when the sender does not have enough balance.
#[test]
fn transfer_balance_too_low() {
    let (
        TestData {
            token_id,
            token_name,
            user_high_token_balance,
            user_no_token_balance,
            ..
        },
        mut runner,
    ) = setup();

    let user_high_token_balance_address = user_high_token_balance.address();
    let user_high_token_initial_balance =
        user_high_token_balance.token_balance(&token_name).unwrap();

    let user_no_token_balance_address = user_no_token_balance.address();

    let transfer_amount = user_high_token_initial_balance + 1;

    runner.execute_transaction(TransactionTestCase {
        input: user_high_token_balance.create_plain_message::<RT, Bank<S>>(CallMessage::Transfer {
            to: user_no_token_balance_address,
            coins: Coins {
                amount: transfer_amount,
                token_id,
            },
        }),
        assert: Box::new(move |result, state| {
            if let TxEffect::Reverted(contents) = result.tx_receipt {
                let Error::ModuleError(err) = contents.reason;
                let mut chain = err.chain();
                let message_1 = chain.next().unwrap().to_string();
                let message_2 = chain.next().unwrap().to_string();
                assert!(chain.next().is_none());
                assert_eq!(
                    format!(
                        "Failed transfer from={} to={} of coins(token_id={} amount={})",
                        user_high_token_balance_address,
                        user_no_token_balance_address,
                        token_id,
                        transfer_amount,
                    ),
                    message_1
                );
                assert_eq!(
                    format!(
                        "Insufficient balance from={}, got={}, needed={}, for token={}",
                        user_high_token_balance_address,
                        user_high_token_initial_balance,
                        transfer_amount,
                        token_name
                    ),
                    message_2,
                );

                assert_eq!(
                    Bank::<S>::default()
                        .get_balance_of(&user_high_token_balance_address, token_id, state)
                        .unwrap_infallible(),
                    Some(user_high_token_initial_balance)
                );

                assert_eq!(
                    Bank::<S>::default()
                        .get_balance_of(&user_no_token_balance_address, token_id, state)
                        .unwrap_infallible(),
                    None
                );
            } else {
                panic!("The transaction should have failed");
            }
        }),
    });
}

/// Test that a transfer call fails when the token does not exist.
#[test]
fn transfer_non_existent_token() {
    let (
        TestData {
            user_high_token_balance: user,
            ..
        },
        mut runner,
    ) = setup();

    let non_existent_token = TestTokenName::new("NonExistentToken".to_string());
    let user_address = user.address();

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<RT, Bank<S>>(CallMessage::Transfer {
            to: user_address,
            coins: Coins {
                amount: 0,
                token_id: non_existent_token.id(),
            },
        }),
        assert: Box::new(move |result, _state| {
            if let TxEffect::Reverted(contents) = result.tx_receipt {
                let Error::ModuleError(err) = contents.reason;
                let mut chain = err.chain();
                let message_1 = chain.next().unwrap().to_string();
                let message_2 = chain.next().unwrap().to_string();
                assert!(chain.next().is_none());
                assert_eq!(
                    format!(
                        "Failed transfer from={} to={} of coins(token_id={} amount={})",
                        user_address,
                        user_address,
                        non_existent_token.id(),
                        0,
                    ),
                    message_1
                );
                assert!(message_2.starts_with(
                    "Value not found for prefix: \"sov_bank/Bank/tokens/\" and storage key:"
                ));
            } else {
                panic!("The transaction should have failed");
            }
        }),
    });
}

/// Test that a transfer call fails when the sender does not have any balance for the token.
#[test]
fn transfer_sender_does_not_have_balance() {
    let (
        TestData {
            token_id,
            user_no_token_balance,
            user_high_token_balance,
            ..
        },
        mut runner,
    ) = setup();

    let sender_address = user_no_token_balance.address();
    let receiver_address = user_high_token_balance.address();
    const TRANSFER_AMOUNT: u64 = 10;

    runner.execute_transaction(TransactionTestCase {
        input: user_no_token_balance.create_plain_message::<RT, Bank<S>>(CallMessage::Transfer {
            to: receiver_address,
            coins: Coins {
                amount: TRANSFER_AMOUNT,
                token_id,
            },
        }),
        assert: Box::new(move |result, _state| {
            if let TxEffect::Reverted(contents) = result.tx_receipt {
                let Error::ModuleError(err) = contents.reason;
                let mut chain = err.chain();
                let message_1 = chain.next().unwrap().to_string();
                let message_2 = chain.next().unwrap().to_string();
                assert!(chain.next().is_none());

                assert_eq!(
                    format!(
                        "Failed transfer from={} to={} of coins(token_id={} amount={})",
                        sender_address, receiver_address, token_id, TRANSFER_AMOUNT,
                    ),
                    message_1
                );

                let expected_message_part = format!(
                    "Value not found for prefix: \"sov_bank/Bank/tokens/{}\" and storage key:",
                    token_id
                );
                assert!(message_2.contains(&expected_message_part));
            } else {
                panic!("The transaction should have failed");
            }
        }),
    });
}

/// Test that a transfer call succeeds even when the receiver does not exist.
#[test]
fn transfer_receiver_does_not_have_balance() {
    let (
        TestData {
            token_id,
            token_name,
            user_high_token_balance,
            ..
        },
        mut runner,
    ) = setup();

    let sender_address = user_high_token_balance.address();
    let sender_initial_balance = user_high_token_balance.token_balance(&token_name).unwrap();

    // Generate a new user with a zero balance
    let receiver_address = TestUser::<S>::generate(0).address();

    const TRANSFER_AMOUNT: u64 = 10;

    runner.execute_transaction(TransactionTestCase {
        input: user_high_token_balance.create_plain_message::<RT, Bank<S>>(CallMessage::Transfer {
            to: receiver_address,
            coins: Coins {
                amount: TRANSFER_AMOUNT,
                token_id,
            },
        }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            assert_eq!(result.events.len(), 1);
            assert_eq!(
                result.events[0],
                TestBankRuntimeEvent::Bank(sov_bank::event::Event::TokenTransferred {
                    from: TokenHolder::User(sender_address),
                    to: TokenHolder::User(receiver_address),
                    coins: Coins {
                        amount: TRANSFER_AMOUNT,
                        token_id
                    }
                })
            );

            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&receiver_address, token_id, state)
                    .unwrap_infallible(),
                Some(TRANSFER_AMOUNT)
            );

            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&sender_address, token_id, state)
                    .unwrap_infallible(),
                Some(sender_initial_balance - TRANSFER_AMOUNT)
            );
        }),
    });
}

/// Test that a transfer call succeeds when the sender and receiver are the same. Even if the sender has zero balance and the transfer amount is NON null.
#[test]
fn transfer_sender_equals_receiver() {
    let (
        TestData {
            token_id,
            user_no_token_balance,
            ..
        },
        mut runner,
    ) = setup();

    let sender_address = user_no_token_balance.address();
    const TRANSFER_AMOUNT: u64 = 10;

    runner.execute_transaction(TransactionTestCase {
        input: user_no_token_balance.create_plain_message::<RT, Bank<S>>(CallMessage::Transfer {
            to: sender_address,
            coins: Coins {
                amount: TRANSFER_AMOUNT,
                token_id,
            },
        }),
        assert: Box::new(move |result, _state| {
            assert!(result.tx_receipt.is_successful());
            assert_eq!(result.events.len(), 1);
            assert_eq!(
                result.events[0],
                TestBankRuntimeEvent::Bank(sov_bank::event::Event::TokenTransferred {
                    from: TokenHolder::User(sender_address),
                    to: TokenHolder::User(sender_address),
                    coins: Coins {
                        amount: TRANSFER_AMOUNT,
                        token_id
                    }
                })
            );
        }),
    });
}

/// Test that a transfer call succeeds when the sender sends a null amount of a valid token. Even if the sender has zero balance.
#[test]
fn transfer_send_zero_amount() {
    let (
        TestData {
            token_id,
            user_no_token_balance,
            user_high_token_balance,
            ..
        },
        mut runner,
    ) = setup();

    let sender_address = user_no_token_balance.address();
    let receiver_address = user_high_token_balance.address();
    const TRANSFER_AMOUNT: u64 = 0;

    runner.execute_transaction(TransactionTestCase {
        input: user_no_token_balance.create_plain_message::<RT, Bank<S>>(CallMessage::Transfer {
            to: receiver_address,
            coins: Coins {
                amount: 0,
                token_id,
            },
        }),
        assert: Box::new(move |result, _state| {
            assert!(result.tx_receipt.is_successful());
            assert_eq!(result.events.len(), 1);
            assert_eq!(
                result.events[0],
                TestBankRuntimeEvent::Bank(sov_bank::event::Event::TokenTransferred {
                    from: TokenHolder::User(sender_address),
                    to: TokenHolder::User(receiver_address),
                    coins: Coins {
                        amount: TRANSFER_AMOUNT,
                        token_id
                    }
                })
            );
        }),
    });
}

/// Test that it is possible to transfer gas tokens
#[test]
fn test_transfer_gas_token() {
    let (
        TestData {
            user_high_token_balance: sender,
            user_no_token_balance: receiver,
            ..
        },
        mut runner,
    ) = setup();

    const TRANSFER_AMOUNT: u64 = 10;
    let sender_address = sender.address();
    let sender_initial_balance = sender.available_gas_balance;

    let receiver_address = receiver.address();
    let receiver_initial_balance = receiver.available_gas_balance;

    runner.execute_transaction(TransactionTestCase {
        input: sender.create_plain_message::<RT, Bank<S>>(CallMessage::Transfer {
            to: receiver_address,
            coins: Coins {
                amount: TRANSFER_AMOUNT,
                token_id: config_gas_token_id(),
            },
        }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            assert_eq!(result.events.len(), 1);
            assert_eq!(
                result.events[0],
                TestBankRuntimeEvent::Bank(sov_bank::event::Event::TokenTransferred {
                    from: TokenHolder::User(sender_address),
                    to: TokenHolder::User(receiver_address),
                    coins: Coins {
                        amount: TRANSFER_AMOUNT,
                        token_id: config_gas_token_id()
                    }
                })
            );

            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&receiver_address, config_gas_token_id(), state)
                    .unwrap_infallible(),
                Some(receiver_initial_balance + TRANSFER_AMOUNT)
            );

            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&sender_address, config_gas_token_id(), state)
                    .unwrap_infallible(),
                Some(sender_initial_balance - result.gas_value_used - TRANSFER_AMOUNT)
            );
        }),
    });
}
