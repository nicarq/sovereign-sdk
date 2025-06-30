#![allow(dead_code)]

use std::sync::Arc;

use futures::StreamExt;
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
    let mut slot_subscription = test_rollup.client.client.subscribe_slots().await.unwrap();

    for _ in 0..30 {
        slot_subscription.next().await;
    }

    let client = test_rollup.client.clone();
    let slot_height = get_height(client).await;

    // Assert the condition that triggers an early return.
    assert!(stop_at_height.get() < slot_height);

    let Err(err) = test_rollup.restart().await else {
        panic!("The rollup should have stopped")
    };

    let mut recods = collector.records();
    assert_eq!(recods.len(), 1);

    let (_, log) = recods.remove(0);
    assert!(log.contains("The requested stop_height"));
    assert!(err.to_string().contains("The requested stop_height"));
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

async fn get_height(client: NodeClient) -> u64 {
    let url = "/modules/chain-state/state/current-heights".to_string();
    let response = client.http_get(&url).await.unwrap();
    let heights: CurrentHeights = serde_json::from_str(&response).unwrap();
    heights.data.value.0
}

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
            minimum_profit_per_tx,
            true,
            max_batch_size,
            BlockProducingConfig::Periodic { block_time_ms: 200 },
            None,
            blob_processing_timeout_secs,
            1,
            MAX_BATCH_EXECUTION_TIME_MILLIS,
            stop_at_rollup_height,
        )
        .await
        .map(|(v, _d)| v.into_iter().next().unwrap()),
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
