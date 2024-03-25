use std::str::FromStr;

use helpers::generate_address;
use sov_bank::{
    get_token_id, Bank, BankConfig, CallMessage, Coins, GasTokenConfig, TokenId,
    TotalSupplyResponse, GAS_TOKEN_ID,
};
use sov_modules_api::{Address, Context, Error, Module, WorkingSet};
use sov_prover_storage_manager::new_orphan_storage;
use sov_state::{DefaultStorageSpec, ProverStorage};

type S = sov_test_utils::TestSpec;
use crate::helpers::create_bank_config_with_token;

mod helpers;

pub type Storage = ProverStorage<DefaultStorageSpec>;

#[test]
fn burn_deployed_tokens() {
    let bank = Bank::<S>::default();
    let tmpdir = tempfile::tempdir().unwrap();
    let mut working_set = WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());

    let sender_address = generate_address("just_sender");
    let sequencer_address = generate_address("sequencer");
    let sender_context = Context::<S>::new(sender_address, sequencer_address, 1);
    let minter_address = generate_address("minter");
    let minter_context = Context::<S>::new(minter_address, sequencer_address, 1);

    let salt = 0;
    let token_name = "Token1".to_owned();
    let initial_balance = 100;
    let token_id = TokenId::from_str(GAS_TOKEN_ID).unwrap();

    let bank_config = BankConfig::<S> {
        gas_token_config: GasTokenConfig {
            token_name,
            address_and_balances: vec![(minter_address, initial_balance)],
            authorized_minters: vec![minter_address],
        },
        tokens: vec![],
    };
    bank.genesis(&bank_config, &mut working_set).unwrap();

    let query_total_supply = |working_set: &mut WorkingSet<S>| -> Option<u64> {
        let total_supply: TotalSupplyResponse =
            bank.supply_of(None, token_id, working_set).unwrap();
        total_supply.amount
    };

    let query_user_balance =
        |user_address: Address, working_set: &mut WorkingSet<S>| -> Option<u64> {
            bank.get_balance_of(user_address, token_id, working_set)
        };

    let previous_total_supply = query_total_supply(&mut working_set);
    assert_eq!(Some(initial_balance), previous_total_supply);

    // -----
    // Burn
    let burn_amount = 10;
    let burn_message = CallMessage::Burn {
        coins: Coins {
            amount: burn_amount,
            token_id,
        },
    };

    bank.call(burn_message.clone(), &minter_context, &mut working_set)
        .expect("Failed to burn token");
    assert_eq!(working_set.events().len(), 1);

    let current_total_supply = query_total_supply(&mut working_set);
    assert_eq!(Some(initial_balance - burn_amount), current_total_supply);
    let minter_balance = query_user_balance(minter_address, &mut working_set);
    assert_eq!(Some(initial_balance - burn_amount), minter_balance);

    let previous_total_supply = current_total_supply;
    // ---
    // Burn by another user, who doesn't have tokens at all
    let failed_to_burn = bank.call(burn_message, &sender_context, &mut working_set);
    assert!(failed_to_burn.is_err());
    let Error::ModuleError(err) = failed_to_burn.err().unwrap();
    let mut chain = err.chain();
    let message_1 = chain.next().unwrap().to_string();
    let message_2 = chain.next().unwrap().to_string();
    assert!(chain.next().is_none());
    assert_eq!(
        format!(
            "Failed to burn coins(token_id={} amount={}) from owner {}",
            token_id, burn_amount, sender_address
        ),
        message_1
    );
    let expected_error_part = format!(
        "Value not found for prefix: \"sov_bank/Bank/tokens/{}\" and storage key:",
        token_id
    );
    assert!(message_2.starts_with(&expected_error_part));

    let current_total_supply = query_total_supply(&mut working_set);
    assert_eq!(previous_total_supply, current_total_supply);
    let sender_balance = query_user_balance(sender_address, &mut working_set);
    assert_eq!(None, sender_balance);

    // ---
    // Allow burning zero tokens
    let burn_zero_message = CallMessage::Burn {
        coins: Coins {
            amount: 0,
            token_id,
        },
    };

    bank.call(burn_zero_message, &minter_context, &mut working_set)
        .expect("Failed to burn token");
    assert_eq!(working_set.events().len(), 2);
    let minter_balance_after = query_user_balance(minter_address, &mut working_set);
    assert_eq!(minter_balance, minter_balance_after);

    // ---
    // Burn more than available
    let burn_message = CallMessage::Burn {
        coins: Coins {
            amount: initial_balance + 10,
            token_id,
        },
    };

    let failed_to_burn = bank.call(burn_message, &minter_context, &mut working_set);
    assert!(failed_to_burn.is_err());
    let Error::ModuleError(err) = failed_to_burn.err().unwrap();
    let mut chain = err.chain();
    let message_1 = chain.next().unwrap().to_string();
    let message_2 = chain.next().unwrap().to_string();
    assert!(chain.next().is_none());
    assert_eq!(
        format!(
            "Failed to burn coins(token_id={} amount={}) from owner {}",
            token_id,
            initial_balance + 10,
            minter_address
        ),
        message_1
    );
    assert_eq!(
        format!("Insufficient funds for {}", minter_address),
        message_2
    );

    // ---
    // Try to burn non-existing token
    let token_id = get_token_id::<S>("NotRealToken2", &minter_address, salt);
    let burn_message = CallMessage::Burn {
        coins: Coins {
            amount: 1,
            token_id,
        },
    };

    let failed_to_burn = bank.call(burn_message, &minter_context, &mut working_set);
    assert!(failed_to_burn.is_err());
    let Error::ModuleError(err) = failed_to_burn.err().unwrap();
    let mut chain = err.chain();
    let message_1 = chain.next().unwrap().to_string();
    let message_2 = chain.next().unwrap().to_string();
    assert!(chain.next().is_none());
    assert_eq!(
        format!(
            "Failed to burn coins(token_id={} amount={}) from owner {}",
            token_id, 1, minter_address
        ),
        message_1
    );
    // Note, no token ID in root cause the message.
    let expected_error_part =
        "Value not found for prefix: \"sov_bank/Bank/tokens/\" and storage key:";
    assert!(message_2.starts_with(expected_error_part));
}

#[test]
fn burn_initial_tokens() {
    let initial_balance = 100;
    let bank_config = create_bank_config_with_token(2, initial_balance);
    let tmpdir = tempfile::tempdir().unwrap();
    let mut working_set = WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    let bank = Bank::default();
    bank.genesis(&bank_config, &mut working_set).unwrap();

    let token_id = TokenId::from_str(sov_bank::GAS_TOKEN_ID).unwrap();
    let sender_address = bank_config.gas_token_config.address_and_balances[0].0;
    let sequencer_address = bank_config.gas_token_config.address_and_balances[1].0;

    let query_user_balance =
        |user_address: Address, working_set: &mut WorkingSet<S>| -> Option<u64> {
            bank.get_balance_of(user_address, token_id, working_set)
        };

    let balance_before = query_user_balance(sender_address, &mut working_set);
    assert_eq!(Some(initial_balance), balance_before);

    let burn_amount = 10;
    let burn_message = CallMessage::Burn {
        coins: Coins {
            amount: burn_amount,
            token_id,
        },
    };

    let context = Context::<S>::new(sender_address, sequencer_address, 1);
    bank.call(burn_message, &context, &mut working_set)
        .expect("Failed to burn token");
    assert_eq!(working_set.events().len(), 1);

    let balance_after = query_user_balance(sender_address, &mut working_set);
    assert_eq!(Some(initial_balance - burn_amount), balance_after);

    // Assume that the rest of edge cases are similar to deployed tokens
}
