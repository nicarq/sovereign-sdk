use sov_modules_api::{Address, Context, Module, PrivateKey, PublicKey, Spec, WorkingSet};
use sov_prover_storage_manager::new_orphan_storage;

use crate::rpc::{self, Response};
use crate::{call, AccountConfig, Accounts};

type S = sov_test_utils::TestSpec;
use sov_test_utils::TestPrivateKey;
#[test]
fn test_config_account() {
    let priv_key = TestPrivateKey::generate();
    let init_pub_key = priv_key.pub_key();
    let init_pub_key_addr = init_pub_key.to_address::<<S as Spec>::Address>();

    let account_config = AccountConfig {
        pub_keys: vec![init_pub_key.clone()],
    };

    let accounts = &mut Accounts::<S>::default();
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());

    accounts.init_module(&account_config, working_set).unwrap();

    let query_response = accounts.get_account(init_pub_key, working_set).unwrap();

    assert_eq!(
        query_response,
        rpc::Response::AccountExists {
            addr: init_pub_key_addr,
            nonce: 0
        }
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
    let sequencer_addr = sequencer.to_address::<<S as Spec>::Address>();
    let sender_context = Context::<S>::new(sender_addr, sequencer_addr, 1);

    // Test new account creation
    {
        let _ = accounts.get_or_create_default(&sender, working_set);

        let query_response = accounts.get_account(sender.clone(), working_set).unwrap();

        assert_eq!(
            query_response,
            rpc::Response::AccountExists {
                addr: sender_addr,
                nonce: 0
            }
        );
    }

    // Test public key update
    {
        let priv_key = TestPrivateKey::generate();
        let new_pub_key = priv_key.pub_key();
        let sig = priv_key.sign(&call::UPDATE_ACCOUNT_MSG);
        accounts
            .call(
                call::CallMessage::<S>::UpdatePublicKey(new_pub_key.clone(), sig),
                &sender_context,
                working_set,
            )
            .unwrap();

        // Account corresponding to the old public key does not exist
        let query_response = accounts.get_account(sender, working_set).unwrap();

        assert_eq!(query_response, rpc::Response::AccountEmpty);

        // New account with the new public key and an old address is created.
        let query_response = accounts.get_account(new_pub_key, working_set).unwrap();

        assert_eq!(
            query_response,
            rpc::Response::AccountExists {
                addr: sender_addr,
                nonce: 0
            }
        );
    }
}

#[test]
fn test_update_account_fails() {
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    let accounts = &mut Accounts::<S>::default();

    let sender_1 = TestPrivateKey::generate().pub_key();
    let sequencer = TestPrivateKey::generate().pub_key();
    let sender_context_1 = Context::<S>::new(sender_1.to_address(), sequencer.to_address(), 1);

    let _ = accounts.get_or_create_default(&sender_1, working_set);

    let priv_key = TestPrivateKey::generate();
    let sender_2 = priv_key.pub_key();
    let sig_2 = priv_key.sign(&call::UPDATE_ACCOUNT_MSG);

    let _ = accounts.get_or_create_default(&sender_2, working_set);

    // The new public key already exists and the call fails.
    assert!(accounts
        .call(
            call::CallMessage::<S>::UpdatePublicKey(sender_2, sig_2),
            &sender_context_1,
            working_set
        )
        .is_err());
}

#[test]
fn test_get_account_after_pub_key_update() {
    let tmpdir = tempfile::tempdir().unwrap();
    let working_set = &mut WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    let accounts = &mut Accounts::<S>::default();

    let sender_1 = TestPrivateKey::generate().pub_key();
    let sequencer = TestPrivateKey::generate().pub_key();
    let sender_1_addr = sender_1.to_address::<<S as Spec>::Address>();
    let sequencer_addr = sequencer.to_address::<<S as Spec>::Address>();
    let sender_context_1 = Context::<S>::new(sender_1_addr, sequencer_addr, 1);

    let _ = accounts.get_or_create_default(&sender_1, working_set);

    let priv_key = TestPrivateKey::generate();
    let new_pub_key = priv_key.pub_key();
    let sig = priv_key.sign(&call::UPDATE_ACCOUNT_MSG);
    accounts
        .call(
            call::CallMessage::<S>::UpdatePublicKey(new_pub_key.clone(), sig),
            &sender_context_1,
            working_set,
        )
        .unwrap();

    let acc = accounts.accounts.get(&new_pub_key, working_set).unwrap();

    assert_eq!(acc.addr, sender_1_addr);
}

#[test]
fn test_response_serialization() {
    let addr: Vec<u8> = (1..=32).collect();
    let nonce = 123456789;
    let mut addr_array = [0u8; 32];
    addr_array.copy_from_slice(&addr);
    let response = Response::AccountExists::<<S as Spec>::Address> {
        addr: Address::from(addr_array),
        nonce,
    };

    let json = serde_json::to_string(&response).unwrap();
    assert_eq!(
        json,
        r#"{"AccountExists":{"addr":"sov1qypqxpq9qcrsszg2pvxq6rs0zqg3yyc5z5tpwxqergd3c8g7rusqqsn6hm","nonce":123456789}}"#
    );
}

#[test]
fn test_response_deserialization() {
    let json = r#"{"AccountExists":{"addr":"sov1qypqxpq9qcrsszg2pvxq6rs0zqg3yyc5z5tpwxqergd3c8g7rusqqsn6hm","nonce":123456789}}"#;
    let response: Response<<S as Spec>::Address> = serde_json::from_str(json).unwrap();

    let expected_addr: Vec<u8> = (1..=32).collect();
    let mut addr_array = [0u8; 32];
    addr_array.copy_from_slice(&expected_addr);
    let expected_response = Response::AccountExists::<<S as Spec>::Address> {
        addr: Address::from(addr_array),
        nonce: 123456789,
    };

    assert_eq!(response, expected_response);
}

#[test]
fn test_response_deserialization_on_wrong_hrp() {
    let json = r#"{"AccountExists":{"addr":"hax1qypqx68ju0l","nonce":123456789}}"#;
    let response: Result<Response<<S as Spec>::Address>, serde_json::Error> =
        serde_json::from_str(json);
    match response {
        Ok(response) => panic!("Expected error, got {:?}", response),
        Err(err) => {
            assert_eq!(err.to_string(), "Wrong HRP: hax at line 1 column 42");
        }
    }
}
