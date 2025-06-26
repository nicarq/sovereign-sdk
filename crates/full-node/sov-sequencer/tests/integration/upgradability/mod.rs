use std::sync::Arc;
use std::time::Duration;

use base64::prelude::BASE64_STANDARD;
use base64::Engine as _;
use sov_api_spec::types::{self as api_types};
use sov_kernels::soft_confirmations::SoftConfirmationsKernel;
use sov_mock_da::BlockProducingConfig;
use sov_mock_zkvm::crypto::private_key::Ed25519PrivateKey;
use sov_modules_api::{RawTx, Runtime};
use sov_test_utils::runtime::genesis::operator::HighLevelOperatorGenesisConfig;
use sov_test_utils::runtime::GenesisParams;
use sov_test_utils::test_rollup::TestRollup;
use sov_test_utils::{
    generate_operator_runtime_with_kernel, RtAgnosticBlueprint, TestAddress, TestSpec, TestUser,
    TEST_BLOB_PROCESSING_TIMEOUT, TEST_MAX_BATCH_SIZE,
};
use sov_value_setter::{ValueSetter, ValueSetterConfig};
use tokio::time::sleep;

use crate::utils::{new_test_rollup, tx_set_value_with_gas, MAX_BATCH_EXECUTION_TIME_MILLIS};
generate_operator_runtime_with_kernel!(kernel_type: SoftConfirmationsKernel<'a, S>, TestRuntime <= value_setter: ValueSetter<S>);
type TestBlueprint = RtAgnosticBlueprint<TestSpec, TestRuntime<TestSpec>>;

#[tokio::test(flavor = "multi_thread")]
async fn tests_sequencer_stop() {
    let (test_rollup, admin) =
        create_test_rollup(0, TEST_MAX_BATCH_SIZE, TEST_BLOB_PROCESSING_TIMEOUT).await;

    let Some(test_rollup) = test_rollup else {
        return;
    };

    test_rollup
        .da_service
        .produce_n_blocks_now(5)
        .await
        .unwrap();
    sleep(Duration::from_millis(200)).await;

    let client = test_rollup.api_client.clone();
    let tx = tx_set_value(&admin.private_key, 0, 7);

    let response = client
        .accept_tx(&api_types::AcceptTxBody {
            body: BASE64_STANDARD.encode(&tx),
        })
        .await
        .unwrap();

    assert_eq!(response.data.events.len(), 1);
}

async fn create_test_rollup(
    minimum_profit_per_tx: u128,
    max_batch_size: usize,
    blob_processing_timeout_secs: u64,
) -> (Option<TestRollup<TestBlueprint>>, TestUser<TestSpec>) {
    let reward_address = TestAddress::new([17; 28]);
    let genesis_config =
        HighLevelOperatorGenesisConfig::<TestSpec>::generate_with_additional_accounts(
            2,
            reward_address,
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
            BlockProducingConfig::Manual,
            None,
            blob_processing_timeout_secs,
            MAX_BATCH_EXECUTION_TIME_MILLIS,
        )
        .await,
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
