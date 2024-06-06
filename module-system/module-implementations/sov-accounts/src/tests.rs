use sov_modules_api::prelude::*;
use sov_modules_api::{Address, Context, CredentialId, Module, PrivateKey, PublicKey};
use sov_prover_storage_manager::new_orphan_storage;
use sov_test_utils::{TestHasher, TestPrivateKey};

use crate::rpc::Response;
use crate::{call, Account, AccountConfig, AccountData, Accounts};

type S = sov_test_utils::TestSpec;

#[test]
fn test_config_account() {
    let priv_key = TestPrivateKey::generate();
    let init_pub_key = &priv_key.pub_key();
    let init_addr = init_pub_key.to_address::<<S as Spec>::Address>();
    let init_credential_id: CredentialId = init_pub_key.credential_id::<TestHasher>();

    let account_config = AccountConfig {
        accounts: vec![AccountData {
            credential_id: init_credential_id,
            address: init_pub_key.into(),
        }],
    };

    let accounts = &mut Accounts::<S>::default();
    let tmpdir = tempfile::tempdir().unwrap();
    let state = &mut WorkingSet::<S>::new(new_orphan_storage(tmpdir.path()).unwrap());

    accounts.init_module(&account_config, state).unwrap();

    let query_response = accounts
        .accounts
        .get(&init_credential_id, state)
        .map(|Account { addr: a }| a);

    assert_eq!(query_response, Some(init_addr));
}

#[test]
fn test_update_account() {
    let tmpdir = tempfile::tempdir().unwrap();
    let state = &mut WorkingSet::<S>::new(new_orphan_storage(tmpdir.path()).unwrap());
    let accounts = &mut Accounts::<S>::default();

    let priv_key = TestPrivateKey::generate();
    let sequencer_priv_key = TestPrivateKey::generate();

    let sender = priv_key.pub_key();
    let sequencer = sequencer_priv_key.pub_key();
    let sender_addr = sender.to_address::<<S as Spec>::Address>();
    let sender_credential_id: CredentialId = sender.credential_id::<TestHasher>();

    let sequencer_addr = sequencer.to_address::<<S as Spec>::Address>();
    let sender_context = Context::<S>::new(sender_addr, Default::default(), sequencer_addr, 1);

    let config = AccountConfig {
        accounts: vec![AccountData {
            credential_id: sender_credential_id,
            address: sender_addr,
        }],
    };

    accounts.init_module(&config, state).unwrap();
    // Test new account creation
    {
        let query_response = accounts
            .accounts
            .get(&sender_credential_id, state)
            .map(|Account { addr: a }| a);

        assert_eq!(query_response, Some(sender_addr));
    }

    // Test credentials id update
    {
        let priv_key = TestPrivateKey::generate();
        let new_pub_key = priv_key.pub_key();
        let new_credential_id: CredentialId = new_pub_key.credential_id::<TestHasher>();
        accounts
            .call(
                call::CallMessage::InsertCredentialId(new_credential_id),
                &sender_context,
                state,
            )
            .unwrap();

        // Account corresponding to the old credential still exists.
        let query_response = accounts
            .accounts
            .get(&sender_credential_id, state)
            .map(|Account { addr: a }| a);

        assert_eq!(query_response, Some(sender_addr));

        // New account with the new public key and an old address is created.
        let query_response = accounts
            .accounts
            .get(&new_credential_id, state)
            .map(|Account { addr: a }| a);

        assert_eq!(query_response, Some(sender_addr));
    }
}

#[test]
fn test_update_account_fails() {
    let tmpdir = tempfile::tempdir().unwrap();
    let state = &mut WorkingSet::<S>::new(new_orphan_storage(tmpdir.path()).unwrap());
    let accounts = &mut Accounts::<S>::default();

    let sender_1 = TestPrivateKey::generate().pub_key();
    let sender_1_addr = sender_1.to_address::<<S as Spec>::Address>();
    let sender_1_credential_id: CredentialId = sender_1.credential_id::<TestHasher>();

    let sequencer = TestPrivateKey::generate().pub_key();
    let sender_context_1 = Context::<S>::new(
        sender_1.to_address(),
        Default::default(),
        sequencer.to_address(),
        1,
    );

    let sender_2 = TestPrivateKey::generate().pub_key();
    let sender_2_addr = sender_2.to_address::<<S as Spec>::Address>();
    let sender_2_credential_id: CredentialId = sender_2.credential_id::<TestHasher>();

    let config = AccountConfig {
        accounts: vec![
            AccountData {
                credential_id: sender_1_credential_id,
                address: sender_1_addr,
            },
            AccountData {
                credential_id: sender_2_credential_id,
                address: sender_2_addr,
            },
        ],
    };

    accounts.init_module(&config, state).unwrap();

    // The new credential already exists and the call fails.
    assert!(accounts
        .call(
            call::CallMessage::InsertCredentialId(sender_2_credential_id),
            &sender_context_1,
            state
        )
        .is_err());
}

#[test]
fn test_get_account_after_pub_key_update() {
    let tmpdir = tempfile::tempdir().unwrap();
    let state = &mut WorkingSet::<S>::new(new_orphan_storage(tmpdir.path()).unwrap());
    let accounts = &mut Accounts::<S>::default();

    let sender = TestPrivateKey::generate().pub_key();
    let sender_addr = sender.to_address::<<S as Spec>::Address>();
    let sender_credential_id: CredentialId = sender.credential_id::<TestHasher>();

    let sequencer = TestPrivateKey::generate().pub_key();
    let sequencer_addr = sequencer.to_address::<<S as Spec>::Address>();
    let sender_context = Context::<S>::new(sender_addr, Default::default(), sequencer_addr, 1);

    let config = AccountConfig {
        accounts: vec![AccountData {
            credential_id: sender_credential_id,
            address: sender_addr,
        }],
    };

    accounts.init_module(&config, state).unwrap();

    let priv_key = TestPrivateKey::generate();
    let new_pub_key = priv_key.pub_key();
    let new_credential_id: CredentialId = new_pub_key.credential_id::<TestHasher>();
    accounts
        .call(
            call::CallMessage::InsertCredentialId(new_credential_id),
            &sender_context,
            state,
        )
        .unwrap();

    let acc = accounts.accounts.get(&new_credential_id, state).unwrap();

    assert_eq!(acc.addr, sender_addr);
}

#[test]
fn test_resolve_sender_address() {
    let tmpdir = tempfile::tempdir().unwrap();
    let state: WorkingSet<S> = WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    let (mut checkpoint, _, _) = state.checkpoint();
    let accounts = &mut Accounts::<S>::default();

    let priv_key = TestPrivateKey::generate();
    let sender = priv_key.pub_key();
    let sender_addr = sender.to_address::<<S as Spec>::Address>();
    let sender_credential_id: CredentialId = sender.credential_id::<TestHasher>();

    let maybe_address =
        accounts.resolve_sender_address(&None, &sender_credential_id, &mut checkpoint);
    assert_eq!(
        maybe_address.unwrap_err().to_string(),
        format!("No default address found for {}", sender_credential_id)
    );

    accounts
        .resolve_sender_address(&Some(sender_addr), &sender_credential_id, &mut checkpoint)
        .unwrap();

    let mut state = checkpoint.to_working_set_unmetered();
    let acc = accounts
        .accounts
        .get(&sender_credential_id, &mut state)
        .unwrap();

    assert_eq!(acc.addr, sender_addr);
}

#[test]
fn test_response_serialization() {
    let addr: Vec<u8> = (1..=32).collect();
    let mut addr_array = [0u8; 32];
    addr_array.copy_from_slice(&addr);
    let response = Response::AccountExists::<<S as Spec>::Address> {
        addr: Address::from(addr_array),
    };

    let json = serde_json::to_string(&response).unwrap();
    assert_eq!(
        json,
        r#"{"AccountExists":{"addr":"sov1qypqxpq9qcrsszg2pvxq6rs0zqg3yyc5z5tpwxqergd3c8g7rusqqsn6hm"}}"#
    );
}

#[test]
fn test_response_deserialization() {
    let json = r#"{"AccountExists":{"addr":"sov1qypqxpq9qcrsszg2pvxq6rs0zqg3yyc5z5tpwxqergd3c8g7rusqqsn6hm"}}"#;
    let response: Response<<S as Spec>::Address> = serde_json::from_str(json).unwrap();

    let expected_addr: Vec<u8> = (1..=32).collect();
    let mut addr_array = [0u8; 32];
    addr_array.copy_from_slice(&expected_addr);
    let expected_response = Response::AccountExists::<<S as Spec>::Address> {
        addr: Address::from(addr_array),
    };

    assert_eq!(response, expected_response);
}

#[test]
fn test_response_deserialization_on_wrong_hrp() {
    let json = r#"{"AccountExists":{"addr":"hax1qypqx68ju0l"}}"#;
    let response: Result<Response<<S as Spec>::Address>, serde_json::Error> =
        serde_json::from_str(json);
    match response {
        Ok(response) => panic!("Expected error, got {:?}", response),
        Err(err) => {
            assert_eq!(err.to_string(), "Wrong HRP: hax at line 1 column 43");
        }
    }
}
