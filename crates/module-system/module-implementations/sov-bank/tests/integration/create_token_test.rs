use sov_bank::{get_token_id, Bank};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::genesis::TestTokenName;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{AsUser, TransactionTestCase};

use crate::helpers::*;

type S = sov_test_utils::TestSpec;

// Check that we can create a token and that the state is correctly updated.
#[test]
fn create_token() {
    let (
        TestData {
            minter,
            user_high_token_balance,
            user_no_token_balance,
            ..
        },
        mut runner,
    ) = setup();

    const INITIAL_TOKEN_BALANCE: u64 = 1000;

    let user_high_token_balance_address = user_high_token_balance.address();
    let user_no_token_balance_address = user_no_token_balance.address();
    let minter_address = minter.as_user().address();
    let token_name = "Token1";
    let token_id = get_token_id::<S>(token_name, &minter_address);

    runner.execute_transaction(TransactionTestCase {
        input: minter.create_plain_message::<Bank<S>>(sov_bank::CallMessage::CreateToken {
            token_name: token_name.try_into().unwrap(),
            initial_balance: INITIAL_TOKEN_BALANCE,
            mint_to_address: user_high_token_balance_address,
            authorized_minters: vec![minter_address],
        }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            assert_eq!(result.events.len(), 1, "There should be one event emitted");
            assert_eq!(
                result.events[0],
                TestBankRuntimeEvent::Bank(sov_bank::event::Event::TokenCreated {
                    token_name: token_name.to_string(),
                    coins: sov_bank::Coins {
                        amount: INITIAL_TOKEN_BALANCE,
                        token_id
                    },
                    minter: sov_bank::utils::TokenHolder::User(user_high_token_balance_address),
                    authorized_minters: vec![sov_bank::utils::TokenHolder::User(minter_address)]
                }),
                "The event should be a TokenCreated event"
            );

            assert_eq!(
                Bank::<S>::default()
                    .get_token_name(&token_id, state)
                    .unwrap(),
                Some(token_name.to_string())
            );

            assert_eq!(
                Bank::<S>::default()
                    .get_total_supply_of(&token_id, state)
                    .unwrap(),
                Some(INITIAL_TOKEN_BALANCE)
            );

            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&user_high_token_balance_address, token_id, state)
                    .unwrap(),
                Some(INITIAL_TOKEN_BALANCE)
            );

            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&minter_address, token_id, state)
                    .unwrap(),
                None,
                "The minter should not receive any tokens! It should only be able to mint"
            );

            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&user_no_token_balance_address, token_id, state)
                    .unwrap(),
                None
            );
        }),
    });
}

/// Check that we can create a token and mint them to a user.
#[test]
fn create_token_and_mint() {
    let (
        TestData {
            minter,
            user_no_token_balance,
            ..
        },
        mut runner,
    ) = setup();

    const INITIAL_TOKEN_BALANCE: u64 = 1000;

    let user_no_token_balance_address = user_no_token_balance.address();
    let minter_address = minter.as_user().address();
    let token_name = "Token1";
    let token_id = get_token_id::<S>(token_name, &minter_address);

    runner.execute_transaction(TransactionTestCase {
        input: minter.create_plain_message::<Bank<S>>(sov_bank::CallMessage::CreateToken {
            token_name: token_name.try_into().unwrap(),
            initial_balance: INITIAL_TOKEN_BALANCE,
            mint_to_address: minter_address,
            authorized_minters: vec![minter_address],
        }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            assert_eq!(result.events.len(), 1, "There should be one event emitted");
            assert_eq!(
                result.events[0],
                TestBankRuntimeEvent::Bank(sov_bank::event::Event::TokenCreated {
                    token_name: token_name.to_string(),
                    coins: sov_bank::Coins {
                        amount: INITIAL_TOKEN_BALANCE,
                        token_id
                    },
                    minter: sov_bank::utils::TokenHolder::User(minter_address),
                    authorized_minters: vec![sov_bank::utils::TokenHolder::User(minter_address)]
                }),
                "The event should be a TokenCreated event"
            );

            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&user_no_token_balance_address, token_id, state)
                    .unwrap(),
                None
            );
        }),
    });

    runner.execute_transaction(TransactionTestCase {
        input: minter.create_plain_message::<Bank<S>>(sov_bank::CallMessage::Mint {
            coins: sov_bank::Coins {
                amount: INITIAL_TOKEN_BALANCE,
                token_id,
            },
            mint_to_address: user_no_token_balance_address,
        }),
        assert: Box::new(move |result, state| {
            assert!(result.tx_receipt.is_successful());
            assert_eq!(result.events.len(), 1, "There should be one event emitted");
            assert_eq!(
                result.events[0],
                TestBankRuntimeEvent::Bank(sov_bank::event::Event::TokenMinted {
                    mint_to_identity: sov_bank::utils::TokenHolder::User(
                        user_no_token_balance_address
                    ),
                    coins: sov_bank::Coins {
                        amount: INITIAL_TOKEN_BALANCE,
                        token_id
                    }
                }),
                "The event should be a TokenMinted event"
            );
            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&user_no_token_balance_address, token_id, state)
                    .unwrap(),
                Some(INITIAL_TOKEN_BALANCE)
            );

            // The minter should have the same balance because the tokens were minted and not transferred
            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&minter_address, token_id, state)
                    .unwrap(),
                Some(INITIAL_TOKEN_BALANCE)
            );

            // The total supply should have increased by the amount of tokens minted
            assert_eq!(
                Bank::<S>::default()
                    .get_total_supply_of(&token_id, state)
                    .unwrap(),
                Some(2 * INITIAL_TOKEN_BALANCE)
            );
        }),
    });
}

#[test]
#[should_panic]
fn overflow_max_supply_genesis_should_panic() {
    let token_name = TestTokenName::new("BankToken".to_string());
    let genesis_config = HighLevelOptimisticGenesisConfig::generate()
        .add_accounts_with_default_balance(1)
        .add_accounts_with_token(&token_name, false, 2, u64::MAX - 2);

    let genesis = GenesisConfig::from_minimal_config(genesis_config.into());

    TestRunner::new_with_genesis(genesis.into_genesis_params(), RT::default());
}
