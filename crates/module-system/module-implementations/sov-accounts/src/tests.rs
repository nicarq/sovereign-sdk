use sov_modules_api::prelude::*;
use sov_modules_api::sov_wallet_format::compiled_schema::CompiledSchema;
use sov_modules_api::{Address, CredentialId};

use crate::query::Response;
use crate::CallMessage;

type S = sov_test_utils::TestSpec;

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

#[test]
fn test_display_accounts_call() {
    #[derive(
        Debug, Clone, PartialEq, borsh::BorshSerialize, sov_modules_api::macros::UniversalWallet,
    )]
    enum RuntimeCall {
        Accounts(CallMessage),
    }

    let msg = RuntimeCall::Accounts(CallMessage::InsertCredentialId(CredentialId([1; 32])));

    let schema = CompiledSchema::of::<RuntimeCall>();
    assert_eq!(
        schema.display(&borsh::to_vec(&msg).unwrap()).unwrap(),
        r#"Accounts.InsertCredentialId(0x0101010101010101010101010101010101010101010101010101010101010101)"#
    );
}
