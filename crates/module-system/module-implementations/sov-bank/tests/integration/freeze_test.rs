use sov_bank::Bank;
use sov_modules_api::{Error, TxEffect};
use sov_test_utils::{AsUser, TransactionTestCase};

use crate::helpers::{setup, TestBankRuntimeEvent, TestData, S};

/// Check that the authorized minter can freeze a token
#[test]
fn freeze_token_happy_path() {
    let (
        TestData {
            minter,
            token_id,
            token_name,
            ..
        },
        mut runner,
    ) = setup();

    let minter_address = minter.as_user().address();

    runner.execute_transaction(TransactionTestCase {
        input: minter.create_plain_message::<Bank<S>>(sov_bank::CallMessage::Freeze { token_id }),
        assert: Box::new(move |result, _| {
            assert_eq!(result.outcome, TxEffect::Successful(()));
            assert_eq!(result.events.len(), 1);
            assert_eq!(
                result.events[0],
                TestBankRuntimeEvent::bank(sov_bank::event::Event::TokenFrozen {
                    freezer: sov_bank::utils::TokenHolder::User(minter_address),
                    token_id
                }),
                "The event should be a TokenFrozen event"
            );
        }),
    });

    // We can check that the token is frozen by trying to mint
    runner.execute_transaction(TransactionTestCase {
        input: minter.create_plain_message::<Bank<S>>(sov_bank::CallMessage::Mint {
            coins: sov_bank::Coins {
                amount: 0,
                token_id,
            },
            mint_to_address: minter_address,
        }),
        assert: Box::new(move |result, _| {
            if let TxEffect::Reverted(Error::ModuleError(err)) = result.outcome {
                let mut chain = err.chain();
                let message_1 = chain.next().unwrap().to_string();
                let message_2 = chain.next().unwrap().to_string();
                assert!(chain.next().is_none());
                assert_eq!(
                    format!(
                        "Failed mint coins(token_id={} amount={}) to {} by authorizer {}",
                        token_id, 0, minter_address, minter_address
                    ),
                    message_1
                );
                assert_eq!(
                    format!("Attempt to mint frozen token {}", token_name),
                    message_2
                );
            } else {
                panic!("The transaction should have reverted");
            }
        }),
    });
}

#[test]
fn freeze_another_time_fails() {
    let (
        TestData {
            minter,
            token_id,
            token_name,
            ..
        },
        mut runner,
    ) = setup();

    let minter_address = minter.as_user().address();

    runner.execute_transaction(TransactionTestCase {
        input: minter.create_plain_message::<Bank<S>>(sov_bank::CallMessage::Freeze { token_id }),
        assert: Box::new(move |result, _| {
            assert_eq!(result.outcome, TxEffect::Successful(()));
        }),
    });

    // We cannot freeze the token again
    runner.execute_transaction(TransactionTestCase {
        input: minter.create_plain_message::<Bank<S>>(sov_bank::CallMessage::Freeze { token_id }),
        assert: Box::new(move |result, _| {
            if let TxEffect::Reverted(Error::ModuleError(err)) = result.outcome {
                let mut chain = err.chain();
                let message_1 = chain.next().unwrap().to_string();
                let message_2 = chain.next().unwrap().to_string();
                assert!(chain.next().is_none());
                assert_eq!(
                    format!(
                        "Failed freeze token_id={} by sender {}",
                        token_id, minter_address
                    ),
                    message_1
                );
                assert_eq!(format!("Token {} is already frozen", token_name), message_2);
            } else {
                panic!("The transaction should have reverted");
            }
        }),
    });
}

#[test]
fn unauthorized_minter_cannot_freeze_token() {
    let (
        TestData {
            user_high_token_balance: unauthorized_user,
            token_id,
            token_name,
            ..
        },
        mut runner,
    ) = setup();

    assert!(!unauthorized_user.is_minter(&token_name));

    let unauthorized_address = unauthorized_user.as_user().address();

    runner.execute_transaction(TransactionTestCase {
        input: unauthorized_user
            .create_plain_message::<Bank<S>>(sov_bank::CallMessage::Freeze { token_id }),
        assert: Box::new(move |result, _| {
            if let TxEffect::Reverted(Error::ModuleError(err)) = result.outcome {
                let mut chain = err.chain();
                let message_1 = chain.next().unwrap().to_string();
                let message_2 = chain.next().unwrap().to_string();
                assert!(chain.next().is_none());
                assert_eq!(
                    format!(
                        "Failed freeze token_id={} by sender {}",
                        token_id, unauthorized_address
                    ),
                    message_1
                );
                assert_eq!(
                    format!(
                        "Sender {} is not an authorized minter of token {}",
                        unauthorized_address, token_name
                    ),
                    message_2
                );
            } else {
                panic!("The transaction should have reverted");
            }
        }),
    });
}
