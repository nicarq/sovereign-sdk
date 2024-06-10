use sov_bank::{get_token_id, Bank, CallMessage};
use sov_modules_api::utils::generate_address;
use sov_modules_api::{Context, Module, WorkingSet};
use sov_prover_storage_manager::new_orphan_storage;

use crate::helpers::*;

type S = sov_test_utils::TestSpec;

#[test]
fn initial_and_deployed_token() {
    let bank_config = create_bank_config_with_token(1, 100);
    let tmpdir = tempfile::tempdir().unwrap();
    let mut working_set = WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    let bank = Bank::default();
    bank.genesis(&bank_config, &mut working_set).unwrap();

    let sender_address = generate_address::<S>("sender");
    let sequencer_address = generate_address::<S>("sequencer");
    let sender_context =
        Context::<S>::new(sender_address, Default::default(), sequencer_address, 1);
    let minter = generate_address::<S>("minter");
    let initial_balance = 500;
    let token_name = "Token1".to_owned();
    let salt = 1;
    let token_id = get_token_id::<S>(&token_name, &sender_address, salt);
    let create_token_message = CallMessage::CreateToken::<S> {
        salt,
        token_name: token_name.clone(),
        initial_balance,
        mint_to_address: minter,
        authorized_minters: vec![minter],
    };

    bank.call(create_token_message, &sender_context, &mut working_set)
        .expect("Failed to create token");

    // Create token event should be present
    assert_eq!(working_set.events().len(), 1);

    let sender_balance = bank.get_balance_of(&sender_address, token_id, &mut working_set);
    assert!(sender_balance.is_none());

    let observed_token_name = bank
        .get_token_name(&token_id, &mut working_set)
        .expect("Token is missing its name");
    assert_eq!(&token_name, &observed_token_name);

    let minter_balance = bank.get_balance_of(&minter, token_id, &mut working_set);
    assert_eq!(Some(initial_balance), minter_balance);

    let total_supply = bank
        .get_total_supply_of(&token_id, &mut working_set)
        .unwrap();
    assert_eq!(initial_balance, total_supply);
}

#[test]
/// Currently integer overflow happens on bank genesis
fn overflow_max_supply() {
    let bank = Bank::<S>::default();
    let tmpdir = tempfile::tempdir().unwrap();
    let mut working_set = WorkingSet::<S>::new(new_orphan_storage(tmpdir.path()).unwrap());

    let bank_config = create_bank_config_with_token(2, u64::MAX - 2);

    let genesis_result = bank.genesis(&bank_config, &mut working_set);
    assert!(genesis_result.is_err());

    assert_eq!(
        "Total supply overflow",
        genesis_result.unwrap_err().to_string()
    );
}
