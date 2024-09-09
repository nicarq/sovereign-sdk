use sov_bank::{Bank, CallMessage, Coins};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{Error, TxEffect};
use sov_test_utils::{AsUser, TransactionTestCase};

use crate::helpers::{setup, TestBankRuntimeEvent, TestData};

type S = sov_test_utils::TestSpec;

/// Tests that a user can mint tokens.
#[test]
fn mint_token_success() {
    let (
        TestData {
            token_id,
            minter,
            user_no_token_balance,
            ..
        },
        mut runner,
    ) = setup();

    const MINT_AMOUNT: u64 = 100;

    runner.execute_transaction(TransactionTestCase {
        input: minter.create_plain_message::<Bank<S>>(CallMessage::Mint {
            coins: Coins {
                amount: MINT_AMOUNT,
                token_id,
            },
            mint_to_address: user_no_token_balance.address(),
        }),
        assert: Box::new(move |result, state| {
            assert_eq!(result.tx_receipt, TxEffect::Successful(()));
            assert_eq!(result.events.len(), 1);
            assert_eq!(
                result.events[0],
                TestBankRuntimeEvent::Bank(sov_bank::event::Event::TokenMinted {
                    mint_to_identity: sov_bank::utils::TokenHolder::User(
                        user_no_token_balance.address()
                    ),
                    coins: Coins {
                        amount: MINT_AMOUNT,
                        token_id
                    }
                })
            );

            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&user_no_token_balance.address(), token_id, state)
                    .unwrap_infallible(),
                Some(MINT_AMOUNT)
            );
        }),
    });
}

#[test]
fn mint_token_fails_if_user_unauthorized() {
    let (
        TestData {
            token_name,
            token_id,
            user_high_token_balance: unauthorized_minter,
            ..
        },
        mut runner,
    ) = setup();

    runner.execute_transaction(TransactionTestCase {
        input: unauthorized_minter.create_plain_message::<Bank<S>>(CallMessage::Mint {
            coins: Coins {
                amount: 0,
                token_id,
            },
            mint_to_address: unauthorized_minter.address(),
        }),
        assert: Box::new(move |result, _state| {
            if let TxEffect::Reverted(Error::ModuleError(err)) = result.tx_receipt {
                let mut chain = err.chain();
                let message_1 = chain.next().unwrap().to_string();
                let message_2 = chain.next().unwrap().to_string();
                assert!(chain.next().is_none());
                assert_eq!(
                    message_1,
                    format!(
                        "Failed mint coins(token_id={} amount={}) to {} by authorizer {}",
                        token_id,
                        0,
                        unauthorized_minter.address(),
                        unauthorized_minter.address(),
                    ),
                );
                assert_eq!(
                    message_2,
                    format!(
                        "Sender {} is not an authorized minter of token {}",
                        unauthorized_minter.address(),
                        token_name
                    ),
                );
            } else {
                panic!("The transaction should have failed");
            }
        }),
    });
}

/// The token originator shouldn't be able to mint tokens he created.
#[test]
fn try_create_token_and_mint_should_fail_if_not_authorized() {
    let (
        TestData {
            token_name,
            token_id,
            user_no_token_balance: user,
            ..
        },
        mut runner,
    ) = setup();

    const INITIAL_BALANCE: u64 = 100;

    runner.execute(
        user.create_plain_message::<Bank<S>>(CallMessage::CreateToken {
            salt: 0,
            token_name: token_name.to_string(),
            initial_balance: 100,
            mint_to_address: user.address(),
            authorized_minters: vec![],
        }),
    );

    runner.execute_transaction(TransactionTestCase {
        input: user.create_plain_message::<Bank<S>>(CallMessage::Mint {
            coins: Coins {
                amount: INITIAL_BALANCE,
                token_id,
            },
            mint_to_address: user.address(),
        }),
        assert: Box::new(move |result, _state| {
            if let TxEffect::Reverted(Error::ModuleError(err)) = result.tx_receipt {
                let mut chain = err.chain();
                let message_1 = chain.next().unwrap().to_string();
                let message_2 = chain.next().unwrap().to_string();
                assert!(chain.next().is_none());
                assert_eq!(
                    message_1,
                    format!(
                        "Failed mint coins(token_id={} amount={}) to {} by authorizer {}",
                        token_id,
                        INITIAL_BALANCE,
                        user.address(),
                        user.address(),
                    ),
                );
                assert_eq!(
                    message_2,
                    format!(
                        "Sender {} is not an authorized minter of token {}",
                        user.address(),
                        token_name
                    ),
                );
            } else {
                panic!("The transaction should have failed");
            }
        }),
    });
}

#[test]
fn mint_token_account_balance_overflow() {
    let (
        TestData {
            token_id, minter, ..
        },
        mut runner,
    ) = setup();

    runner.execute_transaction(TransactionTestCase {
        input: minter.create_plain_message::<Bank<S>>(CallMessage::Mint {
            coins: Coins {
                amount: u64::MAX,
                token_id,
            },
            mint_to_address: minter.address(),
        }),
        assert: Box::new(move |result, _state| {
            if let TxEffect::Reverted(Error::ModuleError(err)) = result.tx_receipt {
                let mut chain = err.chain();
                let message_1 = chain.next().unwrap().to_string();
                let message_2 = chain.next().unwrap().to_string();
                assert!(chain.next().is_none());
                assert_eq!(
                    message_1,
                    format!(
                        "Failed mint coins(token_id={} amount={}) to {} by authorizer {}",
                        token_id,
                        u64::MAX,
                        minter.address(),
                        minter.address(),
                    ),
                );
                assert_eq!(
                    message_2,
                    "Account balance overflow in the mint method of bank module",
                );
            } else {
                panic!("The transaction should have failed");
            }
        }),
    });
}

#[test]
fn mint_token_total_supply_overflow() {
    let (
        TestData {
            token_name,
            token_id,
            minter,
            ..
        },
        mut runner,
    ) = setup();

    let minter_balance = minter.token_balance(&token_name).unwrap();

    runner.execute_transaction(TransactionTestCase {
        input: minter.create_plain_message::<Bank<S>>(CallMessage::Mint {
            coins: Coins {
                amount: u64::MAX - minter_balance - 1,
                token_id,
            },
            mint_to_address: minter.address(),
        }),
        assert: Box::new(move |result, _state| {
            if let TxEffect::Reverted(Error::ModuleError(err)) = result.tx_receipt {
                let mut chain = err.chain();
                let message_1 = chain.next().unwrap().to_string();
                let message_2 = chain.next().unwrap().to_string();
                assert!(chain.next().is_none());
                assert_eq!(
                    message_1,
                    format!(
                        "Failed mint coins(token_id={} amount={}) to {} by authorizer {}",
                        token_id,
                        u64::MAX - minter_balance - 1,
                        minter.address(),
                        minter.address(),
                    ),
                );
                assert_eq!(
                    message_2,
                    "Total Supply overflow in the mint method of bank module",
                );
            } else {
                panic!("The transaction should have failed");
            }
        }),
    });
}
