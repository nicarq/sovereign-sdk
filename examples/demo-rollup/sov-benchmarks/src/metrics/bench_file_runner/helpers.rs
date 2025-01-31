use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::pin::Pin;

use anyhow::ensure;
use demo_stf::runtime::GenesisConfig;
use futures::{Stream, StreamExt};
use sov_benchmarks::{mock_da_risc0_host_args, DEFAULT_FINALIZATION_BLOCKS};
use sov_mock_da::BlockProducingConfig;
use sov_modules_api::capabilities::config_chain_id;
use sov_modules_api::transaction::TxDetails;
use sov_modules_api::{CryptoSpec, HexHash, Runtime, Spec};
use sov_node_client::NodeClient;
use sov_rollup_interface::node::ledger_api::IncludeChildren;
use sov_test_utils::ledger_db::sov_api_spec::types::{AggregatedProof, Slot, TxReceiptResult};
use sov_test_utils::test_rollup::GenesisSource;
use sov_test_utils::{TransactionType, TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE};
use tokio::select;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinHandle;
use tokio::time::sleep;

use super::{BenchLogs, BenchMessage, BenchRollup, BenchRollupBuilder, RT, S};

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
        BlockProducingConfig::Periodic,
        DEFAULT_FINALIZATION_BLOCKS,
    )
    .with_zkvm_host_args(mock_da_risc0_host_args())
    .set_config(|config| {
        config.prover_address = prover_address.to_string();
        config.automatic_batch_production = false;
        config.telegraf_address = telegraf_address;
        config.aggregated_proof_block_jump = 1;
    })
    .set_da_config(|da_config| {
        da_config.sender_address = sequencer_da_address;
        da_config.block_time_ms = 3_000;
    });

    rollup_builder.start().await
}

/// A simple struct that sends batches to the sequencer on behalf of the user
pub struct BatchSender {
    nonces: HashMap<<<S as Spec>::CryptoSpec as CryptoSpec>::PublicKey, u64>,
    /// Channel used to send transactions to wait for to the receiver task
    tx_sender: Sender<HashSet<HexHash>>,
    /// The client used to send transactions to the sequencer
    client: NodeClient,
}

/// The counterpart of the [`BatchSender`] that waits for the results of the transactions sent to the sequencer
/// in a separate thread.
pub struct BatchReceiver {
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
    proof_subscription: Pin<Box<dyn Stream<Item = Result<AggregatedProof, anyhow::Error>> + Send>>,
}

impl BatchReceiver {
    /// Creates a new [`BatchReceiver`].
    pub async fn new(client: NodeClient, tx_channel: Receiver<HashSet<HexHash>>) -> Self {
        Self {
            txs_to_wait_for: Default::default(),
            highest_slot_proven: 0,
            highest_slot_to_prove: 0,
            tx_channel,
            slots_subscription: client
                .client
                .subscribe_finalized_slots_with_children(IncludeChildren::new(true))
                .await
                .expect("Impossible to subscribe to the slots"),
            proof_subscription: client
                .client
                .subscribe_aggregated_proof()
                .await
                .expect("Failed to subscribe to aggregated proofs"),
        }
    }

    /// Starts the receiver thread.
    /// Waits for the results of the transactions sent to the sequencer to be available in the full node.
    pub fn start_receiver(mut self, bench_name: String) -> JoinHandle<anyhow::Result<()>> {
        tokio::spawn(async move {
            loop {
                select! {
                    txs = self.tx_channel.recv(), if !self.tx_channel.is_closed() => {
                        if let Some(txs) = txs {
                            self.txs_to_wait_for.extend(txs);
                        } else {
                            println!("{bench_name}, receiver_task: The channel of transactions to wait for has been closed!");
                        }
                    },

                    maybe_next_slot = self.slots_subscription.next(), if !self.txs_to_wait_for.is_empty() => {
                        let next_slot = maybe_next_slot.ok_or_else(|| anyhow::anyhow!("{bench_name}: The stream of slots has terminated!"))?.map_err(|e| anyhow::anyhow!("{bench_name}: An error occurred while waiting for the next slot! {:?}", e))?;

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

                        println!("{bench_name}, receiver_task: Received a slot from sender task. Still need to wait for {} transactions. Highest slot to prove {}", self.txs_to_wait_for.len(), self.highest_slot_to_prove);
                    },

                    maybe_next_proof = self.proof_subscription.next(), if self.highest_slot_to_prove > self.highest_slot_proven => {
                        maybe_next_proof.ok_or(anyhow::anyhow!("{bench_name}: The stream of proofs has terminated!"))?.map_err(|e| anyhow::anyhow!("{bench_name}: An error occurred while waiting for the next proof! {:?}", e))?;

                        self.highest_slot_proven += 1;

                        println!("{bench_name}, receiver_task: Received a proof. Highest slot proven {} - highest slot to prove {}", self.highest_slot_proven, self.highest_slot_to_prove);
                    },

                    else => {
                        println!("{bench_name}: receiver has completed");
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
    pub async fn new(client: NodeClient, tx_sender: Sender<HashSet<HexHash>>) -> Self {
        Self {
            nonces: Default::default(),
            tx_sender,
            client,
        }
    }

    /// Produces a batch of transactions from bench messages and publishes it to DA through the sequencer.
    pub async fn produce_and_publish_batch(
        &mut self,
        batch: Vec<BenchMessage>,
    ) -> Result<Vec<BenchLogs>, anyhow::Error> {
        /// Maximum number of attempts to publish a batch before giving up.
        const MAX_PUBLICATION_ATTEMPTS: u64 = 4;
        /// Time to wait between publication attempts, this is the initial wait time. After each attempt, the wait time is doubled.
        const WAIT_TIME: std::time::Duration = std::time::Duration::from_millis(2000);

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
            .expect("Failed to send transactions to the sender task");

        let batch_result = {
            let mut curr_wait_time = WAIT_TIME;
            let mut out = None;

            for _ in 0..MAX_PUBLICATION_ATTEMPTS {
                match self
                    .client
                    .client
                    .publish_batch_with_serialized_txs(&txs)
                    .await
                {
                    Ok(batch_result) => {
                        out = Some(batch_result);
                        break;
                    }
                    Err(e) => {
                        println!("An error occurred while trying to publish the batch: {e}. Trying again... \n");
                        sleep(curr_wait_time).await;
                        curr_wait_time *= 2;
                        continue;
                    }
                }
            }

            out
        }
        .ok_or_else(|| {
            anyhow::anyhow!("Failed to publish batch after maximum number of attempts")
        })?;

        ensure!(
            txs.len() == batch_result.tx_hashes.len(),
            "The number of transactions sent should match the number of transactions published by the sequencer. Number sent {}, number published {}",
            txs.len(),
            batch_result.tx_hashes.len()
        );

        for batch_tx_hash in &batch_result.tx_hashes {
            let batch_tx_hash = batch_tx_hash.parse().expect("Impossible to parse tx hash");
            ensure!(
                tx_hashes.contains(&batch_tx_hash),
                "The transaction hash should be included in the batch"
            );
        }

        Ok(outcomes.into_iter().flatten().collect::<Vec<_>>())
    }
}
