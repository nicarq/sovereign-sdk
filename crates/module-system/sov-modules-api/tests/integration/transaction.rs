use sov_modules_api::transaction::{Transaction, UnsignedTransaction};
use sov_test_utils::runtime::TestOptimisticRuntime;
use sov_test_utils::TestSpec;
use sov_universal_wallet::schema::Schema;

type Runtime = TestOptimisticRuntime<TestSpec>;

const ASSERT_MSG: &str = "JSON representation changed, this is a breaking change for web3 SDK, please ensure it is also updated";

#[test]
fn test_unsigned_tx_wallet_serialization_none_gas_limit() {
    let json = r#"{
        "runtime_call": {
            "value_setter": {
                "set_value": 4
            }
        },
        "generation": 2,
        "details": {
            "max_priority_fee_bips": 1,
            "max_fee": 10000,
            "gas_limit": null,
            "chain_id": 1337
        }
    }"#;
    let schema = Schema::of_single_type::<UnsignedTransaction<Runtime, TestSpec>>();

    assert!(schema.json_to_borsh(0, json).is_ok(), "{ASSERT_MSG}");
}

#[test]
fn test_unsigned_tx_wallet_serialization_some_gas_limit() {
    let json = r#"{
        "runtime_call": {
            "value_setter": {
                "set_value": 4
            }
        },
        "generation": 2,
        "details": {
            "max_priority_fee_bips": 1,
            "max_fee": 10000,
            "gas_limit": [500, 500],
            "chain_id": 1337
        }
    }"#;
    let schema = Schema::of_single_type::<UnsignedTransaction<Runtime, TestSpec>>();

    assert!(schema.json_to_borsh(0, json).is_ok(), "{ASSERT_MSG}");
}

#[test]
fn test_tx_wallet_serialization_some_gas_limit() {
    let json = r#"{
        "signature": {
            "msg_sig": [
                197, 161, 16, 121, 196, 253, 39, 80, 96, 211, 6, 131, 61, 32, 48, 100,
                246, 215, 233, 132, 0, 34, 250, 182, 110, 83, 213, 18, 215, 40, 1, 105,
                181, 112, 122, 171, 36, 14, 3, 10, 230, 227, 82, 244, 56, 125, 136, 119,
                117, 39, 34, 216, 127, 24, 21, 220, 112, 100, 195, 138, 80, 59, 62, 2
            ]
        },
        "pub_key": {
            "pub_key": [
                30, 167, 123, 184, 248, 25, 21, 129, 108, 78, 152, 92, 104, 15, 169,
                144, 55, 125, 201, 72, 241, 29, 131, 75, 110, 177, 135, 251, 42, 83,
                204, 230
            ]
        },
        "runtime_call": {
            "value_setter": {
                "set_value": 4
            }
        },
        "generation": 2,
        "details": {
            "max_priority_fee_bips": 1,
            "max_fee": 10000,
            "gas_limit": [500, 500],
            "chain_id": 1337
        }
    }"#;
    let schema = Schema::of_single_type::<Transaction<Runtime, TestSpec>>();

    assert!(schema.json_to_borsh(0, json).is_ok(), "{ASSERT_MSG}");
}

#[test]
fn test_tx_wallet_serialization_none_gas_limit() {
    let json = r#"{
        "signature": {
            "msg_sig": [
                197, 161, 16, 121, 196, 253, 39, 80, 96, 211, 6, 131, 61, 32, 48, 100,
                246, 215, 233, 132, 0, 34, 250, 182, 110, 83, 213, 18, 215, 40, 1, 105,
                181, 112, 122, 171, 36, 14, 3, 10, 230, 227, 82, 244, 56, 125, 136, 119,
                117, 39, 34, 216, 127, 24, 21, 220, 112, 100, 195, 138, 80, 59, 62, 2
            ]
        },
        "pub_key": {
            "pub_key": [
                30, 167, 123, 184, 248, 25, 21, 129, 108, 78, 152, 92, 104, 15, 169,
                144, 55, 125, 201, 72, 241, 29, 131, 75, 110, 177, 135, 251, 42, 83,
                204, 230
            ]
        },
        "runtime_call": {
            "value_setter": {
                "set_value": 4
            }
        },
        "generation": 2,
        "details": {
            "max_priority_fee_bips": 1,
            "max_fee": 10000,
            "gas_limit": null,
            "chain_id": 1337
        }
    }"#;
    let schema = Schema::of_single_type::<Transaction<Runtime, TestSpec>>();

    assert!(schema.json_to_borsh(0, json).is_ok(), "{ASSERT_MSG}");
}
