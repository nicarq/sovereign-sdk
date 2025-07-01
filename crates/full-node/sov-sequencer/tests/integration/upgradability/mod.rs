#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use futures::StreamExt;
use sov_api_spec::types;
use sov_kernels::soft_confirmations::SoftConfirmationsKernel;
use sov_mock_da::BlockProducingConfig;
use sov_mock_zkvm::crypto::private_key::Ed25519PrivateKey;
use sov_modules_api::capabilities::RollupHeight;
use sov_modules_api::{RawTx, Runtime};
use sov_node_client::NodeClient;
use sov_test_utils::logging::LogCollector;
use sov_test_utils::runtime::genesis::operator::HighLevelOperatorGenesisConfig;
use sov_test_utils::runtime::GenesisParams;
use sov_test_utils::test_rollup::TestRollup;
use sov_test_utils::{
    generate_operator_runtime_with_kernel, RtAgnosticBlueprint, TestSpec, TestUser,
    TEST_BLOB_PROCESSING_TIMEOUT, TEST_DEFAULT_USER_BALANCE, TEST_MAX_BATCH_SIZE,
};
use sov_value_setter::{ValueSetter, ValueSetterConfig};
use tracing::Level;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry;

use crate::utils::{new_test_rollup, tx_set_value_with_gas, MAX_BATCH_EXECUTION_TIME_MILLIS};

generate_operator_runtime_with_kernel!(kernel_type: SoftConfirmationsKernel<'a, S>, TestRuntime <= value_setter: ValueSetter<S>);
type TestBlueprint = RtAgnosticBlueprint<TestSpec, TestRuntime<TestSpec>>;

#[tokio::test(flavor = "multi_thread")]
async fn tests_sequencer_stops_if_stop_at_height_too_small() {
    let collector = LogCollector::new(Level::ERROR);
    let subscriber = registry().with(collector.clone());
    subscriber.init();

    let stop_at_height = RollupHeight::new(3);

    let (test_rollup, _) = create_test_rollup(
        0,
        TEST_MAX_BATCH_SIZE,
        TEST_BLOB_PROCESSING_TIMEOUT,
        Some(stop_at_height),
    )
    .await;

    let test_rollup = test_rollup.unwrap();

    test_rollup
        .da_service
        .produce_n_blocks_now(5)
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut slot_subscription = test_rollup.client.client.subscribe_slots().await.unwrap();

    for _ in 0..20 {
        test_rollup.da_service.produce_block_now().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        slot_subscription.next().await;
    }

    let client = test_rollup.client.clone();
    let slot_height = get_height(&client).await;

    // Assert the condition that triggers an early return.
    assert!(stop_at_height < slot_height);

    let Err(err) = test_rollup.restart().await else {
        panic!("The rollup should have stopped")
    };

    let mut recods = collector.records();
    assert_eq!(recods.len(), 1);

    let (_, log) = recods.remove(0);
    assert!(log.contains("The requested stop_height"));
    assert!(err.to_string().contains("The requested stop_height"));
}

#[tokio::test(flavor = "multi_thread")]
async fn tests_sequencer_does_not_accept_tx_after_stop() {
    let stop_at_height = RollupHeight::new(15);

    let (test_rollup, admin) = create_test_rollup(
        0,
        TEST_MAX_BATCH_SIZE,
        TEST_BLOB_PROCESSING_TIMEOUT,
        Some(stop_at_height),
    )
    .await;

    let expected_error = format!(
        "The preferred sequencer has reached the stop height {} and is no longer accepting transactions.",
        stop_at_height.get()
    );

    let test_rollup = test_rollup.unwrap();

    let client = test_rollup.client.clone();

    test_rollup
        .da_service
        .produce_n_blocks_now(10)
        .await
        .unwrap();

    let api_client = test_rollup.api_client.clone();

    let mut nonce = 0;
    let mut current_height = get_height(&client).await;

    let mut slot_subscription = test_rollup.client.client.subscribe_slots().await.unwrap();
    while current_height.get() < stop_at_height.get() + 10 {
        test_rollup.da_service.produce_block_now().await.unwrap();
        slot_subscription.next().await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        current_height = get_height(&client).await;

        let tx_update_one = tx_set_value(&admin.private_key, nonce, 8);
        let res = api_client
            .accept_tx(&types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx_update_one),
            })
            .await;

        nonce += 1;

        // All transactions should be accepted until the stop height is reached.
        if current_height <= stop_at_height {
            match res {
                Ok(_) => (),
                Err(err) => panic!("Unexpected error: {err:?}"),
            }
        }

        // After the stop height, the sequencer should not accept any transactions.
        if current_height > stop_at_height {
            let err = res.unwrap_err();

            match err {
                sov_api_spec::Error::InvalidResponsePayload(bytes, _) => {
                    let err_str = String::from_utf8_lossy(&bytes);
                    assert!(err_str.contains(&expected_error));
                }
                _ => panic!("Unexpected error: {err:?}"),
            }
        }
    }

    let _ = test_rollup.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_rollup_operates_only_on_finalized_blocks_if_stop_at_height_set() {
    let stop_at_height = RollupHeight::new(15);

    let (test_rollup, _) = create_test_rollup(
        0,
        TEST_MAX_BATCH_SIZE,
        TEST_BLOB_PROCESSING_TIMEOUT,
        Some(stop_at_height),
    )
    .await;

    let test_rollup = test_rollup.unwrap();

    let client = test_rollup.client.clone();
    assert_rollup_processes_only_finalized_blocks(&client).await;

    test_rollup
        .da_service
        .produce_n_blocks_now(10)
        .await
        .unwrap();
    assert_rollup_processes_only_finalized_blocks(&client).await;

    let mut current_height = get_height(&client).await;
    let mut slot_subscription = test_rollup.client.client.subscribe_slots().await.unwrap();
    while current_height.get() < stop_at_height.get() + 10 {
        test_rollup.da_service.produce_block_now().await.unwrap();
        slot_subscription.next().await;
        // We nned to wait a bit so the new blcok is visible to the sequencer.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_rollup_processes_only_finalized_blocks(&client).await;
        current_height = get_height(&client).await;
    }

    let _ = test_rollup.shutdown().await;
}

async fn assert_rollup_processes_only_finalized_blocks(client: &NodeClient) {
    let last_finalized_block_height = get_last_finalized_block_height(client).await;
    let last_block_height = get_last_block_height(client).await;
    // During upgrade procedure rollup processes only finalized blocks.
    assert_eq!(last_finalized_block_height, last_block_height);
}

use serde::Deserialize;

#[derive(Deserialize, Debug)]
struct CurrentHeights {
    data: Data,
    meta: Meta,
}

#[derive(Deserialize, Debug)]
struct Data {
    value: (u64, u64),
}

#[derive(Deserialize, Debug)]
struct Meta {}

async fn get_height(client: &NodeClient) -> RollupHeight {
    let url = "/modules/chain-state/state/current-heights";
    let response = client.http_get(url).await.unwrap();
    let heights: CurrentHeights = serde_json::from_str(&response).unwrap();
    RollupHeight::new(heights.data.value.0)
}

async fn get_last_finalized_block_height(client: &NodeClient) -> u64 {
    get_block_height(client, true).await
}

async fn get_last_block_height(client: &NodeClient) -> u64 {
    get_block_height(client, false).await
}

async fn get_block_height(client: &NodeClient, finalized: bool) -> u64 {
    let url = if finalized {
        "/ledger/slots/finalized"
    } else {
        "/ledger/slots/latest"
    };
    let response = client.http_get(url).await.unwrap();
    let height: sov_api_spec::types::GetLatestSlotResponse =
        serde_json::from_str(&response).unwrap();
    height.data.unwrap().number
}

#[allow(clippy::too_many_arguments)]
async fn create_test_rollup(
    minimum_profit_per_tx: u128,
    max_batch_size: usize,
    blob_processing_timeout_secs: u64,
    stop_at_rollup_height: Option<RollupHeight>,
) -> (Option<TestRollup<TestBlueprint>>, TestUser<TestSpec>) {
    let reward_user = TestUser::<TestSpec>::generate(TEST_DEFAULT_USER_BALANCE);

    let genesis_config =
        HighLevelOperatorGenesisConfig::<TestSpec>::generate_with_additional_accounts(
            2,
            reward_user,
        );

    let admin = genesis_config.additional_accounts()[0].clone();
    let rt_genesis_config =
        <TestRuntime<TestSpec> as Runtime<TestSpec>>::GenesisConfig::from_minimal_config(
            genesis_config.into(),
            ValueSetterConfig {
                admin: admin.address(),
            },
        );

    let genesis_params = GenesisParams {
        runtime: rt_genesis_config.clone(),
    };

    let dir = Arc::new(tempfile::tempdir().unwrap());

    (
        new_test_rollup::<TestRuntime<TestSpec>>(
            dir.clone(),
            genesis_params.runtime.sequencer_registry.seq_da_address,
            genesis_params,
            3,
            minimum_profit_per_tx,
            true,
            max_batch_size,
            BlockProducingConfig::Manual,
            None,
            blob_processing_timeout_secs,
            1,
            MAX_BATCH_EXECUTION_TIME_MILLIS,
            stop_at_rollup_height,
        )
        .await
        .map(|v| v.into_iter().next().unwrap()),
        admin,
    )
}

fn tx_set_value(key: &Ed25519PrivateKey, nonce: u64, value_to_set: u64) -> RawTx {
    tx_set_value_with_gas::<TestRuntime<TestSpec>>(
        key,
        nonce,
        value_to_set,
        None,
        sov_test_utils::TEST_DEFAULT_MAX_FEE,
    )
}
