use core::time::Duration;
use std::sync::Arc;

use demo_stf::runtime::{Runtime, RuntimeCall};
use futures::StreamExt;
use sov_blob_storage::config_deferred_slots_count;
use sov_cli::NodeClient;
use sov_demo_rollup::{mock_da_risc0_host_args, MockDemoRollup};
use sov_mock_da::{BlockProducingConfig, MockAddress, MockDaSpec};
use sov_modules_api::execution_mode::Native;
use sov_modules_api::transaction::{PriorityFeeBips, Transaction, UnsignedTransaction};
use sov_modules_api::{CryptoSpec, OperatingMode, RawTx, Spec};
use sov_modules_macros::config_value;
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::node::ledger_api::IncludeChildren;
use sov_test_utils::test_rollup::{read_private_key, RollupBuilder};

use crate::test_helpers::{test_genesis_source, DemoRollupSpec, CHAIN_HASH};

type TestSpec = DemoRollupSpec;
type TestPrivateKey = <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::PrivateKey;

const MAX_TX_FEE: u64 = 100_000_000;
const UNREGISTERED_SENDER: MockAddress = MockAddress::new([121; 32]);
const MINIMUM_BOND: u64 = 100_000_000;
const ESTIMATED_BLOCK_PROCESSING_TIME: Duration = Duration::from_millis(200);
const FINALIZATION_BLOCKS: u32 = 1;

// Verifies that a rollup with a preferred sequencer can handle forced registration from a different DA address.
// Steps:
// 1. Start the rollup with the preferred batch builder and demo-stf.
// 2. Submit a forced registration from another DA address.
// 3. Wait until the forced registration is processed.
// 4. Use the REST API to confirm that the new sequencer is registered.
#[tokio::test(flavor = "multi_thread")]
async fn flaky_test_forced_sequencer_registration() -> anyhow::Result<()> {
    let rollup = RollupBuilder::<MockDemoRollup<Native>>::new(
        test_genesis_source(OperatingMode::Zk),
        BlockProducingConfig::Periodic,
        FINALIZATION_BLOCKS,
    )
    .with_zkvm_host_args(mock_da_risc0_host_args())
    .set_config(|c| {
        c.automatic_batch_production = true;
        c.rollup_prover_config = None;
        c.max_channel_size = 1;
        c.max_infos_in_db = 1;
    })
    .set_da_config(|c| {
        // We need to set the block producing mode to periodic to ensure that the forced registration
        // eventually succeeds because the registration batch is deferred.
        c.block_producing = BlockProducingConfig::Periodic;
        c.block_time_ms = ESTIMATED_BLOCK_PROCESSING_TIME
            .as_millis()
            .try_into()
            .unwrap();
    })
    .start()
    .await
    .unwrap();

    let da_service = Arc::new(
        rollup
            .da_service
            .another_on_the_same_layer(UNREGISTERED_SENDER),
    );

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

    let _receipt = da_service
        .send_transaction(&blob, fee)
        .await
        .await?
        .expect("Failed to submit forced sequencer registration to DA");

    let wait_for_slots = config_deferred_slots_count() + FINALIZATION_BLOCKS as u64 + 5;

    let mut slots = client
        .client
        .subscribe_finalized_slots_with_children(IncludeChildren::new(true))
        .await?;

    for _i in 0..wait_for_slots {
        let _slot = slots
            .next()
            .await
            .transpose()?
            .expect("slot data is missing");
        // This optimization could be enabled, when update of API state on sequencer side will be stable
        // if !slot.batches.is_empty() {
        //     break;
        // }
    }

    let allowed_sequencer = client
        .sequencer_rollup_address::<TestSpec, MockDaSpec>(&UNREGISTERED_SENDER)
        .await?
        .expect("Allowed sequencer response should contain data");
    assert_eq!(allowed_sequencer.balance, MINIMUM_BOND);
    assert_eq!(allowed_sequencer.address, key_and_address.address);
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
