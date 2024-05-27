use sov_modules_api::prelude::*;
use sov_modules_api::transaction::{AuthenticatedTransactionData, PriorityFeeBips};
use sov_modules_api::{Address, Context, CredentialId, Gas, Module, PrivateKey, PublicKey};
use sov_prover_storage_manager::new_orphan_storage;
use sov_test_utils::{TestHasher, TestPrivateKey};

use crate::rpc::{self, Response};
use crate::{call, AccountConfig, AccountData, Accounts};

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
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());

    accounts.init_module(&account_config, working_set).unwrap();

    let query_response = accounts
        .get_account(init_credential_id, working_set)
        .unwrap();

    assert_eq!(
        query_response,
        rpc::Response::AccountExists { addr: init_addr }
    );
}

#[test]
fn test_update_account() {
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
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

    accounts.init_module(&config, working_set).unwrap();
    // Test new account creation
    {
        let query_response = accounts
            .get_account(sender_credential_id, working_set)
            .unwrap();

        assert_eq!(
            query_response,
            rpc::Response::AccountExists { addr: sender_addr }
        );
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
                working_set,
            )
            .unwrap();

        // Account corresponding to the old credential still exists.
        let query_response = accounts
            .get_account(sender_credential_id, working_set)
            .unwrap();

        assert_eq!(
            query_response,
            rpc::Response::AccountExists { addr: sender_addr }
        );

        // New account with the new public key and an old address is created.
        let query_response = accounts
            .get_account(new_credential_id, working_set)
            .unwrap();

        assert_eq!(
            query_response,
            rpc::Response::AccountExists { addr: sender_addr }
        );
    }
}

#[test]
fn test_update_account_fails() {
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::<S>::new(new_orphan_storage(tmpdir.path()).unwrap());
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

    accounts.init_module(&config, working_set).unwrap();

    // The new credential already exists and the call fails.
    assert!(accounts
        .call(
            call::CallMessage::InsertCredentialId(sender_2_credential_id),
            &sender_context_1,
            working_set
        )
        .is_err());
}

#[test]
fn test_get_account_after_pub_key_update() {
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::<S>::new(new_orphan_storage(tmpdir.path()).unwrap());
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

    accounts.init_module(&config, working_set).unwrap();

    let priv_key = TestPrivateKey::generate();
    let new_pub_key = priv_key.pub_key();
    let new_credential_id: CredentialId = new_pub_key.credential_id::<TestHasher>();
    accounts
        .call(
            call::CallMessage::InsertCredentialId(new_credential_id),
            &sender_context,
            working_set,
        )
        .unwrap();

    let acc = accounts
        .accounts
        .get(&new_credential_id, working_set)
        .unwrap();

    assert_eq!(acc.addr, sender_addr);
}

#[test]
fn test_resolve_sender_address() {
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set: WorkingSet<S> = WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    let (mut checkpoint, _, _) = working_set.checkpoint();
    let accounts = &mut Accounts::<S>::default();

    let priv_key = TestPrivateKey::generate();
    let sender = priv_key.pub_key();
    let sender_addr = sender.to_address::<<S as Spec>::Address>();
    let sender_credential_id: CredentialId = sender.credential_id::<TestHasher>();

    let tx = create_test_tx::<S>(None, sender_credential_id);

    let maybe_address = accounts.resolve_sender_address(&tx, &mut checkpoint);
    assert_eq!(
        maybe_address.unwrap_err().to_string(),
        format!("No default address found for {}", sender_credential_id)
    );

    let tx = create_test_tx::<S>(Some(sender_addr), sender_credential_id);
    accounts
        .resolve_sender_address(&tx, &mut checkpoint)
        .unwrap();

    let mut working_set = checkpoint.to_working_set_unmetered();
    let acc = accounts
        .accounts
        .get(&sender_credential_id, &mut working_set)
        .unwrap();

    assert_eq!(acc.addr, sender_addr);
}

fn create_test_tx<S: Spec>(
    sender_addr: Option<S::Address>,
    sender_credential_id: CredentialId,
) -> AuthenticatedTransactionData<S> {
    AuthenticatedTransactionData::<S> {
        credentials: Default::default(),
        credential_id: sender_credential_id,
        default_address: sender_addr,
        chain_id: 0,
        max_priority_fee_bips: PriorityFeeBips::ZERO,
        max_fee: 0,
        gas_limit: Some(<S as Spec>::Gas::zero()),
        nonce: 0,
    }
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
