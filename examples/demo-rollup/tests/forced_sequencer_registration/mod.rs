use core::time::Duration;
use std::sync::Arc;

use demo_stf::runtime::{Runtime, RuntimeCall};
use sov_cli::NodeClient;
use sov_demo_rollup::{mock_da_risc0_host_args, MockDemoRollup};
use sov_mock_da::{BlockProducingConfig, MockAddress, MockDaConfig, MockDaSpec};
use sov_modules_api::execution_mode::Native;
use sov_modules_api::transaction::{PriorityFeeBips, Transaction, UnsignedTransaction};
use sov_modules_api::{CryptoSpec, OperatingMode, RawTx, Spec};
use sov_modules_macros::config_value;
use sov_rollup_interface::node::da::DaService;
use sov_stf_runner::processes::RollupProverConfig;
use sov_test_utils::test_rollup::{read_private_key, RollupBuilder};
use tokio::time::sleep;

use crate::test_helpers::{test_genesis_source, DemoRollupSpec, CHAIN_HASH};

type TestSpec = DemoRollupSpec;
type TestPrivateKey = <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::PrivateKey;

const MAX_TX_FEE: u64 = 100_000_000;
const UNREGISTERED_SENDER: MockAddress = MockAddress::new([121; 32]);
const MINIMUM_BOND: u64 = 100_000_000;
const ESTIMATED_BLOCK_PROCESSING_TIME: Duration = Duration::from_millis(100);

#[tokio::test(flavor = "multi_thread")]
async fn flaky_test_forced_sequencer_registration() -> anyhow::Result<()> {
    // We need to set the block producing mode to periodic to ensure that the forced registration
    // eventually succeed because the registration batch is deferred.
    let mut da_config = MockDaConfig::instant_with_sender(UNREGISTERED_SENDER);
    da_config.block_producing = BlockProducingConfig::Periodic;
    da_config.block_time_ms = ESTIMATED_BLOCK_PROCESSING_TIME
        .as_millis()
        .try_into()
        .unwrap();

    let rollup = RollupBuilder::<MockDemoRollup<Native>>::new(
        test_genesis_source(OperatingMode::Zk),
        BlockProducingConfig::Periodic,
        1,
    )
    .with_zkvm_host_args(mock_da_risc0_host_args())
    .with_standard_batch_builder()
    .set_config(|c| {
        c.automatic_batch_production = false; // FIXME(@neysofu): finish migrating all tests off of manual batch production.
        c.rollup_prover_config = RollupProverConfig::Skip;
        c.max_channel_size = 1;
        c.max_infos_in_db = 1;
    })
    .set_da_config(|c| {
        *c = da_config;
    })
    .start()
    .await
    .unwrap();

    let da_service = rollup.da_service.clone();

    tokio::select! {
        err = rollup.rollup_task => err??,
        res = forced_sequencer_registration_test_case(da_service, &rollup.client) => res?,
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

    da_service
        .send_transaction(&blob, fee)
        .await
        .await?
        .unwrap();

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
