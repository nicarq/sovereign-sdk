mod helpers;

use helpers::*;
use sov_bank::{
    get_token_id, Bank, BankConfig, CallMessage, Coins, GasTokenConfig, TotalSupplyResponse,
};
use sov_modules_api::utils::generate_address;
use sov_modules_api::{Address, Context, Error, Module, WorkingSet};
use sov_prover_storage_manager::new_orphan_storage;
use sov_state::{DefaultStorageSpec, ProverStorage};

type S = sov_test_utils::TestSpec;
pub type Storage = ProverStorage<DefaultStorageSpec>;

#[test]
fn transfer_initial_token() {
    let initial_balance = 100;
    let transfer_amount = 10;
    let bank_config = create_bank_config_with_token(4, initial_balance);
    let token_name = bank_config.gas_token_config.token_name.clone();
    let tmpdir = tempfile::tempdir().unwrap();
    let mut working_set = WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    let bank = Bank::default();
    bank.genesis(&bank_config, &mut working_set).unwrap();

    let token_id = sov_bank::GAS_TOKEN_ID;
    let sender_address = bank_config.gas_token_config.address_and_balances[0].0;
    let receiver_address = bank_config.gas_token_config.address_and_balances[1].0;
    let sequencer_address = bank_config.gas_token_config.address_and_balances[3].0;
    assert_ne!(sender_address, receiver_address);

    // Preparation
    let query_user_balance =
        |user_address: Address, working_set: &mut WorkingSet<S>| -> Option<u64> {
            bank.get_balance_of(user_address, token_id, working_set)
        };

    let query_total_supply = |working_set: &mut WorkingSet<S>| -> Option<u64> {
        let total_supply: TotalSupplyResponse =
            bank.supply_of(None, token_id, working_set).unwrap();
        total_supply.amount
    };

    let sender_balance_before = query_user_balance(sender_address, &mut working_set);
    let receiver_balance_before = query_user_balance(receiver_address, &mut working_set);
    let total_supply_before = query_total_supply(&mut working_set);
    assert!(total_supply_before.is_some());

    assert_eq!(Some(initial_balance), sender_balance_before);
    assert_eq!(sender_balance_before, receiver_balance_before);
    let sender_context = Context::<S>::new(sender_address, sequencer_address, 1);
    // Transfer happy test
    {
        let transfer_message = CallMessage::Transfer {
            to: receiver_address,
            coins: Coins {
                amount: transfer_amount,
                token_id,
            },
        };

        bank.call(transfer_message, &sender_context, &mut working_set)
            .expect("Transfer call failed");
        // 1 event for transfer, since creation and initial balance is part of genesis
        assert_eq!(working_set.events().len(), 1);

        let sender_balance_after = query_user_balance(sender_address, &mut working_set);
        let receiver_balance_after = query_user_balance(receiver_address, &mut working_set);

        assert_eq!(
            Some(initial_balance - transfer_amount),
            sender_balance_after
        );
        assert_eq!(
            Some(initial_balance + transfer_amount),
            receiver_balance_after
        );
        let total_supply_after = query_total_supply(&mut working_set);
        assert_eq!(total_supply_before, total_supply_after);
    }

    // Balance is too low.
    {
        let transfer_message = CallMessage::Transfer {
            to: receiver_address,
            coins: Coins {
                amount: initial_balance + 1,
                token_id,
            },
        };
        let result = bank.call(transfer_message, &sender_context, &mut working_set);
        assert!(result.is_err());
        let Error::ModuleError(err) = result.err().unwrap();
        let mut chain = err.chain();
        let message_1 = chain.next().unwrap().to_string();
        let message_2 = chain.next().unwrap().to_string();
        let message_3 = chain.next().unwrap().to_string();
        assert!(chain.next().is_none());
        assert_eq!(
            format!(
                "Failed transfer from={} to={} of coins(token_id={} amount={})",
                sender_address,
                receiver_address,
                token_id,
                initial_balance + 1,
            ),
            message_1
        );
        assert_eq!(
            format!(
                "Incorrect balance on={} for token={}",
                sender_address, token_name
            ),
            message_2,
        );
        assert_eq!(
            format!("Insufficient funds for {}", sender_address),
            message_3,
        );
    }

    // Non existent token
    {
        let salt = 13;
        let token_name = "NonExistingToken".to_owned();
        let token_id = get_token_id::<S>(&token_name, &sender_address, salt);

        let transfer_message = CallMessage::Transfer {
            to: receiver_address,
            coins: Coins {
                amount: 1,
                token_id,
            },
        };

        let result = bank.call(transfer_message, &sender_context, &mut working_set);
        assert!(result.is_err());
        let Error::ModuleError(err) = result.err().unwrap();
        let mut chain = err.chain();
        let message_1 = chain.next().unwrap().to_string();
        let message_2 = chain.next().unwrap().to_string();
        assert!(chain.next().is_none());
        assert_eq!(
            format!(
                "Failed transfer from={} to={} of coins(token_id={} amount={})",
                sender_address, receiver_address, token_id, 1,
            ),
            message_1
        );
        assert!(message_2
            .starts_with("Value not found for prefix: \"sov_bank/Bank/tokens/\" and storage key:"));
    }

    // Sender does not exist
    {
        let unknown_sender = generate_address::<S>("non_existing_sender");
        let sequencer = generate_address::<S>("sequencer");
        let unknown_sender_context = Context::<S>::new(unknown_sender, sequencer, 1);

        let sender_balance = query_user_balance(unknown_sender, &mut working_set);
        assert!(sender_balance.is_none());

        let receiver_balance_before = query_user_balance(receiver_address, &mut working_set);

        let transfer_message = CallMessage::Transfer {
            to: receiver_address,
            coins: Coins {
                amount: 1,
                token_id,
            },
        };

        let result = bank.call(transfer_message, &unknown_sender_context, &mut working_set);
        assert!(result.is_err());
        let Error::ModuleError(err) = result.err().unwrap();
        let mut chain = err.chain();
        let message_1 = chain.next().unwrap().to_string();
        let message_2 = chain.next().unwrap().to_string();
        let message_3 = chain.next().unwrap().to_string();
        assert!(chain.next().is_none());

        assert_eq!(
            format!(
                "Failed transfer from={} to={} of coins(token_id={} amount={})",
                unknown_sender, receiver_address, token_id, 1,
            ),
            message_1
        );
        assert_eq!(
            format!(
                "Incorrect balance on={} for token={}",
                unknown_sender, token_name
            ),
            message_2,
        );

        let expected_message_part = format!(
            "Value not found for prefix: \"sov_bank/Bank/tokens/{}\" and storage key:",
            token_id
        );

        assert!(message_3.contains(&expected_message_part));

        let receiver_balance_after = query_user_balance(receiver_address, &mut working_set);
        assert_eq!(receiver_balance_before, receiver_balance_after);
    }

    // Receiver does not exist (this should succeed)
    {
        let unknown_receiver = generate_address::<S>("non_existing_receiver");

        let receiver_balance_before = query_user_balance(unknown_receiver, &mut working_set);
        assert!(receiver_balance_before.is_none());

        let transfer_message = CallMessage::Transfer {
            to: unknown_receiver,
            coins: Coins {
                amount: 1,
                token_id,
            },
        };

        bank.call(transfer_message, &sender_context, &mut working_set)
            .expect("Transfer call failed");
        // Num transfer events should be 2
        assert_eq!(working_set.events().len(), 2);
        let receiver_balance_after = query_user_balance(unknown_receiver, &mut working_set);
        assert_eq!(Some(1), receiver_balance_after);
    }

    // Sender equals receiver (this call should succeed)
    {
        let total_supply_before = query_total_supply(&mut working_set);
        let sender_balance_before = query_user_balance(sender_address, &mut working_set);
        assert!(sender_balance_before.is_some());

        let transfer_message = CallMessage::Transfer {
            to: sender_address,
            coins: Coins {
                amount: 1,
                token_id,
            },
        };
        let resp = bank.call(transfer_message, &sender_context, &mut working_set);
        assert!(resp.is_ok());
        // Num transfers should still be 3 since the sender = receiver case should succeed
        assert_eq!(working_set.events().len(), 3);

        let sender_balance_after = query_user_balance(sender_address, &mut working_set);
        assert_eq!(sender_balance_before, sender_balance_after);
        let total_supply_after = query_total_supply(&mut working_set);
        assert_eq!(total_supply_after, total_supply_before);
    }
}

#[test]
fn transfer_deployed_token() {
    let bank = Bank::<S>::default();
    let tmpdir = tempfile::tempdir().unwrap();
    let mut working_set = WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());

    let sender_address = generate_address::<S>("just_sender");
    let receiver_address = generate_address::<S>("just_receiver");
    let sequencer_address = generate_address::<S>("just_sequencer");

    let token_name = "Token1".to_owned();
    let initial_balance = 1000;
    let token_id = sov_bank::GAS_TOKEN_ID;

    let bank_config = BankConfig::<S> {
        gas_token_config: GasTokenConfig {
            token_name: token_name.clone(),
            authorized_minters: vec![sender_address],
            address_and_balances: vec![(sender_address, initial_balance)],
        },
        tokens: vec![],
    };
    bank.genesis(&bank_config, &mut working_set).unwrap();

    assert_ne!(sender_address, receiver_address);

    // Preparation
    let query_user_balance =
        |user_address: Address, working_set: &mut WorkingSet<S>| -> Option<u64> {
            bank.get_balance_of(user_address, token_id, working_set)
        };

    let query_total_supply = |working_set: &mut WorkingSet<S>| -> Option<u64> {
        let total_supply: TotalSupplyResponse =
            bank.supply_of(None, token_id, working_set).unwrap();
        total_supply.amount
    };

    let total_supply_before = query_total_supply(&mut working_set);
    assert!(total_supply_before.is_some());

    let sender_balance_before = query_user_balance(sender_address, &mut working_set);
    let receiver_balance_before = query_user_balance(receiver_address, &mut working_set);

    assert_eq!(Some(initial_balance), sender_balance_before);
    assert!(receiver_balance_before.is_none());

    let transfer_amount = 15;
    let transfer_message = CallMessage::Transfer {
        to: receiver_address,
        coins: Coins {
            amount: transfer_amount,
            token_id,
        },
    };

    let sender_context = Context::<S>::new(sender_address, sequencer_address, 1);
    bank.call(transfer_message, &sender_context, &mut working_set)
        .expect("Transfer call failed");
    // Transfer token event should be present
    assert_eq!(working_set.events().len(), 1);

    let sender_balance_after = query_user_balance(sender_address, &mut working_set);
    let receiver_balance_after = query_user_balance(receiver_address, &mut working_set);

    assert_eq!(
        Some(initial_balance - transfer_amount),
        sender_balance_after
    );
    assert_eq!(Some(transfer_amount), receiver_balance_after);
    let total_supply_after = query_total_supply(&mut working_set);
    assert_eq!(total_supply_before, total_supply_after);
}
