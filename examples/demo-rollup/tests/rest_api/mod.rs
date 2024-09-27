use std::str::FromStr;

use anyhow::Context;
use demo_stf::genesis_config::GenesisPaths;
use demo_stf::runtime::RuntimeCall;
use demo_stf_json_client::types::AnyJsonValue;
use futures::StreamExt;
use serde::Deserialize;
use sov_bank::GAS_TOKEN_ID;
use sov_cli::wallet_state::PrivateKeyAndAddress;
use sov_kernels::basic::BasicKernelGenesisPaths;
use sov_mock_da::{BlockProducingConfig, MockDaSpec};
use sov_modules_api::rest::utils::ResponseObject;
use sov_rollup_interface::common::HexHash;
use sov_test_utils::{default_test_signed_transaction, TestSpec};

use crate::test_helpers::{get_appropriate_rollup_prover_config, read_private_keys, TestRollup};

#[derive(Debug, Deserialize)]
struct ValueResponse {
    value: [u64; 2],
}

#[tokio::test(flavor = "multi_thread")]
async fn trailing_slashes_handled() -> anyhow::Result<()> {
    let test_rollup = TestRollup::create_test_rollup(
        get_appropriate_rollup_prover_config(),
        BlockProducingConfig::OnSubmit,
        0,
        GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
        BasicKernelGenesisPaths {
            chain_state: "../test-data/genesis/integration-tests/chain_state_zk.json".into(),
        },
    )
    .await?;

    let response = test_rollup
        .client
        .query_rest_endpoint::<ResponseObject<ValueResponse>>(
            "/modules/attester-incentives/state/minimum-challenger-bond",
        )
        .await?;

    let bond = response.data.unwrap().value;

    let response = test_rollup
        .client
        .query_rest_endpoint::<ResponseObject<ValueResponse>>(
            "/modules/attester-incentives/state/minimum-challenger-bond/",
        )
        .await?;

    assert_eq!(Some(bond), response.data.map(|d| d.value));

    let swagger_ui_url_1 = test_rollup.client.http_get("/swagger-ui").await?;
    let swagger_ui_url_2 = test_rollup.client.http_get("/swagger-ui/").await?;

    assert_eq!(swagger_ui_url_1, swagger_ui_url_2);

    Ok(())
}

async fn setup() -> anyhow::Result<demo_stf_json_client::Client> {
    let test_rollup = TestRollup::create_test_rollup(
        get_appropriate_rollup_prover_config(),
        BlockProducingConfig::OnSubmit,
        0,
        GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
        BasicKernelGenesisPaths {
            chain_state: "../test-data/genesis/integration-tests/chain_state_zk.json".into(),
        },
    )
    .await?;

    // Based on an assumption that this key is admin in sov-value-setter
    let key_and_address = read_private_keys::<TestSpec>("tx_signer_private_key.json");
    let msg = RuntimeCall::<TestSpec, MockDaSpec>::ValueSetter(
        sov_value_setter::CallMessage::SetManyValues(vec![1, 2, 3, 4, 5, 6, 7, 8]),
    );

    let tx = default_test_signed_transaction(&key_and_address.private_key, &msg, 0);
    let mut slot_subscription = test_rollup.client.ledger.subscribe_slots().await?;
    test_rollup
        .client
        .sequencer
        .publish_batch_with_serialized_txs(&[tx])
        .await?;
    slot_subscription.next().await;

    let url = format!("{}/modules", &test_rollup.client.base_url);
    Ok(demo_stf_json_client::Client::new(&url))
}

#[tokio::test(flavor = "multi_thread")]
async fn test_runtime_spec_with_gen_client() -> anyhow::Result<()> {
    let runtime_client = setup().await?;

    check_base_runtime_info(&runtime_client)
        .await
        .context("Base runtime check")?;
    check_state_value(&runtime_client)
        .await
        .context("StateValue")?;
    check_state_map(&runtime_client).await.context("StateMap")?;
    check_state_vec(&runtime_client).await.context("StateVec")?;
    check_custom_endpoints(&runtime_client)
        .await
        .context("Custom endpoints")?;
    check_historical_data(&runtime_client)
        .await
        .context("Historical data")?;

    Ok(())
}

async fn check_base_runtime_info(client: &demo_stf_json_client::Client) -> anyhow::Result<()> {
    //Get the list of modules
    let modules = client.get_modules().await.context("Modules list")?;
    let modules = modules.data.clone().unwrap().modules.unwrap().0;

    // There are some modules, but to make test break less we have loose check that some data has been put in the response.
    assert!(modules.len() > 5);

    let bank = modules.get("bank").unwrap().clone();
    assert!(bank.id.starts_with("module_1"));

    let bank_module_info = client
        .get_module("bank")
        .await
        .context("Bank module info")?;
    let bank_module_info = bank_module_info.data.clone().unwrap();
    let module_name = bank_module_info.name.unwrap();
    assert_eq!("Bank", module_name);
    assert!(bank_module_info.id.unwrap().starts_with("module_1"));
    assert!(bank_module_info.prefix.unwrap().starts_with("0x"));
    assert!(!bank_module_info.description.unwrap().is_empty());

    let state_items = bank_module_info.state_items;

    let state_item_keys: Vec<String> = state_items.keys().cloned().collect();
    assert_eq!(vec!["tokens"], state_item_keys);

    let value = state_items.get("tokens").unwrap();

    assert_eq!(
        Some(demo_stf_json_client::types::Namespace::User),
        value.namespace
    );

    assert_eq!(Some("state_map"), value.type_.as_deref());
    assert!(value.prefix.clone().unwrap().starts_with("0x"));
    assert!(!value.description.clone().unwrap().is_empty());

    // TODO: Query unknown module. https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1351

    Ok(())
}

async fn check_state_value(client: &demo_stf_json_client::Client) -> anyhow::Result<()> {
    // Known value: u64
    let finality_period = client
        .attester_incentives_rollup_finality_period_get_state_value(None)
        .await?;
    let finality_period = if let AnyJsonValue::Object(inner) = &finality_period.data {
        let value = inner
            .get("value")
            .cloned()
            .expect("finality period must be set");
        value.as_u64().expect("finality period must be a number")
    } else {
        panic!(
            "Unexpected type of finality period response: {:?}",
            &finality_period.data
        );
    };
    assert!(finality_period >= 1);

    // Known value: array
    let sequencer_bond = client
        .sequencer_registry_minimum_bond_get_state_value(None)
        .await?;

    let sequencer_bond = if let AnyJsonValue::Object(inner) = &sequencer_bond.data {
        let value = inner
            .get("value")
            .cloned()
            .expect("minimum sequencer bond must be set");
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_u64().unwrap())
            .collect::<Vec<u64>>()
    } else {
        panic!(
            "Unexpected type of minimum sequencer bond response: {:?}",
            &sequencer_bond.data
        );
    };
    assert_eq!(vec![500000, 500000], sequencer_bond);

    // Empty value
    let empty_value = client.value_setter_value_get_state_value(None).await?;
    match &empty_value.data {
        AnyJsonValue::Object(inner) => {
            let value = inner.get("value");
            assert_eq!(Some(&serde_json::Value::Null), value);
        }
        _ => panic!("Unexpected type for non set StateValue"),
    }

    Ok(())
}

async fn check_state_map(client: &demo_stf_json_client::Client) -> anyhow::Result<()> {
    // State Map meta info
    let meta_info = client
        .sequencer_registry_allowed_sequencers_get_state_map_info(None)
        .await?;

    let meta_info = meta_info.data.clone();

    assert!(!meta_info.description.unwrap().is_empty());
    assert!(meta_info.prefix.unwrap().starts_with("0x"));
    assert_eq!(Some("state_map"), meta_info.type_.as_deref());
    assert_eq!(
        Some(demo_stf_json_client::types::Namespace::User),
        meta_info.namespace
    );

    // Get the state map: known value.
    let token_deployer = read_private_keys::<TestSpec>("token_deployer_private_key.json");
    let credential_id_response = client
        .accounts_credential_ids_get_state_map_element(&token_deployer.address.to_string(), None)
        .await?;

    if let AnyJsonValue::String(k) = credential_id_response.data.key.clone() {
        assert_eq!(token_deployer.address.to_string(), k);
    } else {
        panic!(
            "StateMap: unexpected key type: {:?}",
            &credential_id_response.data.key
        );
    }
    if let AnyJsonValue::Array(credentials) = &credential_id_response.data.value {
        assert_eq!(1, credentials.len());
        let credential_jsoned = credentials.first().unwrap().clone();
        let credential_id = credential_jsoned.as_str().unwrap().to_string();
        let _credential_id = HexHash::from_str(&credential_id)?;
    } else {
        panic!(
            "StateMap: unexpected value type: {:?}",
            &credential_id_response.data.value
        );
    }

    // Unknown value
    let unknown = PrivateKeyAndAddress::<TestSpec>::generate();
    let _credential_id_response = client
        .accounts_credential_ids_get_state_map_element(&unknown.address.to_string(), None)
        .await
        .unwrap_err();
    // TODO: Known value, parameter is "to_string".
    // TODO: Check error types. https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1351
    Ok(())
}

async fn check_state_vec(client: &demo_stf_json_client::Client) -> anyhow::Result<()> {
    let state_vec_info = client
        .value_setter_many_values_get_state_vec_info(None)
        .await
        .context("vector info")?;
    let info = state_vec_info.data.clone().length.unwrap();
    assert_eq!(8, info);

    // TODO: Check value

    let state_vec_element_0 = client
        .value_setter_many_values_get_state_vec_element(0, None)
        .await
        .context("first element")?;

    let state_vec_element_1 = client
        .value_setter_many_values_get_state_vec_element(1, None)
        .await?;
    let state_vec_element_last = client
        .value_setter_many_values_get_state_vec_element(7, None)
        .await
        .context("last element")?;

    let value_0_json = state_vec_element_0.data.clone().value;
    let value_1_json = state_vec_element_1.data.clone().value;
    let value_last_json = state_vec_element_last.data.clone().value;

    match (value_0_json, value_1_json, value_last_json) {
        (
            AnyJsonValue::Number(value_0),
            AnyJsonValue::Number(value_1),
            AnyJsonValue::Number(value_last),
        ) => {
            assert_eq!(1.0, value_0);
            assert_eq!(2.0, value_1);
            assert_eq!(8.0, value_last);
        }
        (_, _, _) => panic!("Incorrect type returned in vector"),
    }

    let _state_vec_out_of_bounds = client
        .value_setter_many_values_get_state_vec_element(u16::MAX as u64, None)
        .await
        .unwrap_err();
    // TODO: Check error types. https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1351

    Ok(())
}

async fn check_custom_endpoints(client: &demo_stf_json_client::Client) -> anyhow::Result<()> {
    let gas_token_id = demo_stf_json_client::types::TokenId::from_str(&GAS_TOKEN_ID.to_string())?;
    let total_gas_supply = client
        .bank_custom_token_get_total_supply(&gas_token_id)
        .await?;
    let coins = total_gas_supply.data.clone().unwrap();
    assert!(coins.amount.unwrap() > 100);

    assert_eq!(gas_token_id, coins.token_id.unwrap());

    // Unknown user
    let unknown = PrivateKeyAndAddress::<TestSpec>::generate();
    let address = demo_stf_json_client::types::Address(unknown.address.to_string());
    let _balance = client
        .bank_custom_token_get_user_balance(&gas_token_id, &address)
        .await
        .unwrap_err();
    // TODO: Check error types. https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1351

    Ok(())
}

async fn check_historical_data(client: &demo_stf_json_client::Client) -> anyhow::Result<()> {
    let state_vec_info = client
        .value_setter_many_values_get_state_vec_info(Some(0))
        .await?;
    let info = state_vec_info.data.clone().length.unwrap();
    assert_eq!(0, info);

    // TODO: Returns head value, should be 404 https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1351
    // let state_vec_info = client
    //     .value_setter_many_values_get_state_vec_info(Some(u32::MAX as u64))
    //     .await
    //     .unwrap_err();
    // TODO: Check error types. https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1351
    // println!("E: {:?}", state_vec_info);
    Ok(())
}
