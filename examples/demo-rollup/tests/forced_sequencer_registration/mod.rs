use core::time::Duration;
use std::sync::Arc;

use demo_stf::runtime::{Runtime, RuntimeCall};
use sov_cli::NodeClient;
use sov_demo_rollup::MockDemoRollup;
use sov_mock_da::{BlockProducingConfig, MockAddress, MockDaConfig, MockDaSpec};
use sov_modules_api::execution_mode::Native;
use sov_modules_api::transaction::{PriorityFeeBips, Transaction, UnsignedTransaction};
use sov_modules_api::{OperatingMode, RawTx};
use sov_modules_macros::config_value;
use sov_rollup_interface::node::da::DaService;
use sov_sequencer::BatchBuilderMode;
use sov_stf_runner::processes::RollupProverConfig;
use sov_test_utils::test_rollup::{read_private_key, RollupBuilder};
use sov_test_utils::{TestPrivateKey, TestSpec};
use tokio::time::sleep;

use crate::test_helpers::{test_genesis_source, CHAIN_HASH};

const MAX_TX_FEE: u64 = 100_000_000;
const UNREGISTERED_SENDER: MockAddress = MockAddress::new([121; 32]);
const MINIMUM_BOND: u64 = 100_000_000;
const ESTIMATED_BLOCK_PROCESSING_TIME: Duration = Duration::from_millis(100);

#[tokio::test(flavor = "multi_thread")]
async fn flaky_test_forced_sequencer_registration() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let (rpc_port_tx, _rpc_port_rx) = tokio::sync::oneshot::channel();
    let (rest_port_tx, rest_port_rx) = tokio::sync::oneshot::channel();

    // We need to set the block producing mode to periodic to ensure that the forced registration
    // eventually succeed because the registration batch is deferred.
    let mut da_config = MockDaConfig::instant_with_sender(UNREGISTERED_SENDER);
    da_config.block_producing = BlockProducingConfig::Periodic;
    da_config.block_time_ms = ESTIMATED_BLOCK_PROCESSING_TIME
        .as_millis()
        .try_into()
        .unwrap();

    let rollup = RollupBuilder::<MockDemoRollup<Native>>::construct_rollup(
        temp_dir,
        test_genesis_source(OperatingMode::Zk),
        RollupProverConfig::Skip,
        da_config,
        1,
        1,
        1,
        BatchBuilderMode::Standard(Default::default()),
    )
    .await;
    let da_service = rollup.runner.da_service();

    let rollup_task = tokio::spawn(async move {
        rollup
            .run_and_report_addr(Some(rpc_port_tx), Some(rest_port_tx))
            .await
            .unwrap();
    });

    let rest_port = rest_port_rx.await?.port();
    let client = NodeClient::new_at_localhost(rest_port).await?;

    tokio::select! {
        err = rollup_task => err?,
        res = forced_sequencer_registration_test_case(da_service, &client) => res?,
    };
    Ok(())
}

async fn forced_sequencer_registration_test_case(
    da_service: Arc<impl DaService>,
    client: &NodeClient,
) -> anyhow::Result<()> {
    let key_and_address = read_private_key::<TestSpec>("tx_signer_private_key.json");
    let key = key_and_address.private_key;

    let tx = build_register_sequencer_tx(&key, 0);
    let blob = transaction_into_blob(tx);

    let fee = da_service.estimate_fee(blob.len()).await.unwrap();

    da_service.send_transaction(&blob, fee).await.unwrap();

    // We are waiting until enough time has passed to ensure that the registration batch is executed.
    sleep(Duration::from_millis(
        (ESTIMATED_BLOCK_PROCESSING_TIME.as_millis() * config_value!("DEFERRED_SLOTS_COUNT") * 2)
            .try_into()
            .unwrap(),
    ))
    .await;

    let allowed_sequencer_response = client
        .sequencer_rollup_address::<TestSpec, MockDaSpec>(&UNREGISTERED_SENDER)
        .await?;

    assert!(allowed_sequencer_response.is_some());

    Ok(())
}

fn build_register_sequencer_tx(
    key: &TestPrivateKey,
    nonce: u64,
) -> Transaction<Runtime<TestSpec>, TestSpec> {
    let msg =
        RuntimeCall::<TestSpec>::SequencerRegistry(sov_sequencer_registry::CallMessage::Register {
            da_address: UNREGISTERED_SENDER,
            amount: MINIMUM_BOND,
        });
    let chain_id = config_value!("CHAIN_ID");
    let max_priority_fee_bips = PriorityFeeBips::ZERO;
    let max_fee = MAX_TX_FEE;
    let gas_limit = None;
    Transaction::<Runtime<TestSpec>, TestSpec>::new_signed_tx(
        key,
        &CHAIN_HASH,
        UnsignedTransaction::new(
            msg,
            chain_id,
            max_priority_fee_bips,
            max_fee,
            nonce,
            gas_limit,
        ),
    )
}

fn transaction_into_blob(transaction: Transaction<Runtime<TestSpec>, TestSpec>) -> Vec<u8> {
    let tx_data = borsh::to_vec(&transaction).unwrap();
    let blob_data = RawTx { data: tx_data };
    borsh::to_vec(&blob_data).unwrap()
}
