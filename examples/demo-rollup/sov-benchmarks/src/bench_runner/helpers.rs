use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::ensure;
use demo_stf::runtime::GenesisConfig;
use futures::{Stream, StreamExt};
use sov_mock_da::storable::service::StorableMockDaService;
use sov_mock_da::BlockProducingConfig;
use sov_modules_api::capabilities::config_chain_id;
use sov_modules_api::transaction::TxDetails;
use sov_modules_api::{CryptoSpec, HexHash, Runtime, Spec};
use sov_node_client::NodeClient;
use sov_rollup_interface::node::ledger_api::IncludeChildren;
use sov_test_utils::ledger_db::sov_api_spec::types::{Slot, TxReceiptResult};
use sov_test_utils::test_rollup::GenesisSource;
use sov_test_utils::{TransactionType, TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE};
use tokio::select;
use tokio::sync::broadcast;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinHandle;
use tracing::{info, trace};

use super::{BenchLogs, BenchMessage, BenchRollup, BenchRollupBuilder, RT, S};
use crate::{mock_da_risc0_host_args, DEFAULT_FINALIZATION_BLOCKS};

/// Setups the rollup for the benchmarks.
/// We give the maximum possible gas balance to the prover and sequencer to ensure that they can pay for the transactions.
pub async fn setup_rollup(
    genesis_config: GenesisConfig<S>,
    telegraf_address: SocketAddr,
) -> anyhow::Result<BenchRollup> {
    let sequencer_da_address = genesis_config.sequencer_registry.seq_da_address;
    let prover_address = genesis_config
        .prover_incentives
        .initial_provers
        .first()
        .unwrap()
        .0;

    let rollup_builder = BenchRollupBuilder::new(
        GenesisSource::CustomParams(genesis_config.into_genesis_params()),
        BlockProducingConfig::Manual,
        DEFAULT_FINALIZATION_BLOCKS,
    )
    .with_zkvm_host_args(mock_da_risc0_host_args())
    .set_config(|config| {
        config.prover_address = prover_address.to_string();
        config.automatic_batch_production = true;
        config.telegraf_address = telegraf_address;
        config.aggregated_proof_block_jump = 1;

        // This value should be greater than the number of slots we want to run as part of the benchmark.
        config.max_channel_size = 1_500;
        config.max_infos_in_db = 1_500;
    })
    .set_da_config(|da_config| {
        da_config.sender_address = sequencer_da_address;
    });

    rollup_builder.start().await
}

/// A simple struct that sends batches to the sequencer on behalf of the user
pub struct BatchSender {
    /// The name of the benchmark to execute
    bench_name: String,
    /// The nonces used to send transactions
    nonces: HashMap<<<S as Spec>::CryptoSpec as CryptoSpec>::PublicKey, u64>,
    /// Channel used to send transactions to wait for to the receiver task
    tx_sender: Sender<HashSet<HexHash>>,
    /// The client used to send transactions to the sequencer
    client: NodeClient,
}

/// The counterpart of the [`BatchSender`] that waits for the results of the transactions sent to the sequencer
/// in a separate thread.
pub struct BatchReceiver {
    /// The name of the benchmark to execute
    bench_name: String,
    /// A list of transactions sent we are waiting for inclusion on DA
    txs_to_wait_for: HashSet<HexHash>,
    /// The highest slot to prove
    highest_slot_to_prove: u64,
    /// The highest slot number proven so far.
    highest_slot_proven: u64,
    /// Channel used to receive transactions to wait for to the sender task
    tx_channel: Receiver<HashSet<HexHash>>,
    /// For now we use a slot subscription until we can reliably receive [`TxStatus::Processed`] from the full-node
    slots_subscription: Pin<Box<dyn Stream<Item = Result<Slot, anyhow::Error>> + Send>>,
    /// We use a proof subscription to know how far we have generated proofs
    proof_subscription: broadcast::Receiver<()>,
    /// The DA service used to send transactions to the sequencer
    da_service: Arc<StorableMockDaService>,
}

impl BatchReceiver {
    /// Creates a new [`BatchReceiver`].
    pub async fn new(
        bench_name: String,
        client: NodeClient,
        tx_channel: Receiver<HashSet<HexHash>>,
        da_service: &Arc<StorableMockDaService>,
    ) -> Self {
        Self {
            bench_name,
            txs_to_wait_for: Default::default(),
            highest_slot_proven: 0,
            highest_slot_to_prove: 0,
            tx_channel,
            slots_subscription: client
                .client
                .subscribe_finalized_slots_with_children(IncludeChildren::new(true))
                .await
                .expect("Impossible to subscribe to the slots"),
            proof_subscription: da_service.subscribe_proof_posted(),
            da_service: da_service.clone(),
        }
    }

    /// Starts the receiver thread.
    /// Waits for the results of the transactions sent to the sequencer to be available in the full node.
    pub fn start_receiver(mut self) -> JoinHandle<anyhow::Result<()>> {
        tokio::spawn(async move {
            loop {
                select! {
                    txs = self.tx_channel.recv(), if !self.tx_channel.is_closed() => {
                        if let Some(txs) = txs {
                            self.txs_to_wait_for.extend(txs);
                        }
                    },

                    maybe_next_slot = self.slots_subscription.next(), if !self.txs_to_wait_for.is_empty() => {
                        let next_slot = maybe_next_slot.ok_or_else(|| anyhow::anyhow!("{}: The stream of slots has terminated!", self.bench_name))?.map_err(|e| anyhow::anyhow!("{}: An error occurred while waiting for the next slot! {:?}", self.bench_name, e))?;

                        for batch in next_slot.batches {
                            for tx in batch.txs {
                                let parsed_hash: HexHash =
                                    tx.hash.parse().expect("Impossible to parse tx_hash!");


                                if self.txs_to_wait_for.contains(&parsed_hash) {
                                    assert_eq!(
                                        tx.receipt.result,
                                        TxReceiptResult::Successful,
                                        "The transaction should be successful"
                                    );

                                    self.txs_to_wait_for.remove(&parsed_hash);

                                    if next_slot.number > self.highest_slot_to_prove {
                                        self.highest_slot_to_prove = next_slot.number;
                                    }
                                }
                            }
                        }

                        trace!(bench = self.bench_name, thread = "receiver", txs_to_wait_for = self.txs_to_wait_for.len(), highest_slot_to_prove = self.highest_slot_to_prove, "Received a slot from sender task.");
                    },

                    maybe_next_proof = self.proof_subscription.recv(), if self.highest_slot_to_prove > self.highest_slot_proven => {
                        maybe_next_proof.map_err(|e| anyhow::anyhow!("{}: An error occurred while waiting for the next proof! {:?}", self.bench_name, e))?;

                        self.highest_slot_proven += 1;

                        info!(bench = self.bench_name, thread = "receiver", higest_slot_proven = self.highest_slot_proven, highest_slot_to_prove = self.highest_slot_to_prove, "Received a proof");

                        // We need to produce a block to ensure that the proof is included in the DA layer.
                        self.da_service.produce_block_now().await?;
                    },

                    else => {
                        info!(bench = self.bench_name, thread = "receiver", "receiver has completed");
                        break;
                    }
                }
            }

            Ok(())
        })
    }
}

impl BatchSender {
    /// Creates a new [`BatchSender`].
    pub async fn new(
        bench_name: String,
        client: NodeClient,
        tx_sender: Sender<HashSet<HexHash>>,
    ) -> Self {
        Self {
            bench_name,
            nonces: Default::default(),
            tx_sender,
            client,
        }
    }

    /// Produces a batch of transactions from bench messages and publishes it to DA through the sequencer.
    pub async fn send_txs_to_sequencer(
        &mut self,
        batch: Vec<BenchMessage>,
    ) -> Result<Vec<BenchLogs>, anyhow::Error> {
        let (txs, outcomes): (Vec<_>, Vec<_>) = batch
            .into_iter()
            .map(|output| {
                (
                    TransactionType::<RT, S>::sign(
                        output.message,
                        output.sender,
                        &RT::CHAIN_HASH,
                        TxDetails {
                            max_priority_fee_bips: TEST_DEFAULT_MAX_PRIORITY_FEE,
                            max_fee: TEST_DEFAULT_MAX_FEE,
                            gas_limit: None,
                            chain_id: config_chain_id(),
                        },
                        &mut self.nonces,
                    ),
                    output.outcome.unwrap_changes(),
                )
            })
            .unzip();

        let tx_hashes = txs.iter().map(|tx| tx.hash()).collect::<HashSet<_>>();

        self.tx_sender
            .send(tx_hashes.clone())
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "{}: Failed to send transactions to the receiver task. Error {:?}",
                    self.bench_name, err
                )
            });

        let batch_hashes = self
            .client
            .client
            .send_txs_to_sequencer(&txs)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to publish batch. Error {e}"))?
            .iter()
            .map(|val| val.data.id.clone())
            .collect::<Vec<_>>();

        ensure!(
            txs.len() == batch_hashes.len(),
            "{}: The number of transactions sent should match the number of transactions published by the sequencer. Number sent {}, number published {}",
            self.bench_name,
            txs.len(),
            batch_hashes.len()
        );

        for tx_hash in &batch_hashes {
            let tx_hash = tx_hash.parse().expect("Impossible to parse tx hash");
            ensure!(
                tx_hashes.contains(&tx_hash),
                "{}: The transaction hash should be included in the batch",
                self.bench_name
            );
        }

        Ok(outcomes.into_iter().flatten().collect::<Vec<_>>())
    }
}
