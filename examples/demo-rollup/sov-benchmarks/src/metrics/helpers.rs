use std::collections::{HashMap, HashSet};
use std::pin::Pin;

use anyhow::ensure;
use demo_stf::runtime::GenesisConfig;
use futures::{Stream, TryStreamExt};
use sov_benchmarks::{
    mock_da_risc0_host_args, DEFAULT_BLOCK_PRODUCING_CONFIG, DEFAULT_FINALIZATION_BLOCKS,
};
use sov_modules_api::capabilities::config_chain_id;
use sov_modules_api::transaction::TxDetails;
use sov_modules_api::{CryptoSpec, HexHash, Runtime, Spec};
use sov_node_client::NodeClient;
use sov_rollup_interface::node::ledger_api::IncludeChildren;
use sov_test_utils::ledger_db::sov_api_spec::types::{Slot, TxReceiptResult};
use sov_test_utils::test_rollup::GenesisSource;
use sov_test_utils::{TransactionType, TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE};
use tokio::time::sleep;

use crate::bench_runner::{BenchLogs, BenchMessage, BenchRollup, BenchRollupBuilder, RT, S};

/// Setups the rollup for the benchmarks.
/// We give the maximum possible gas balance to the prover and sequencer to ensure that they can pay for the transactions.
pub async fn setup(genesis_config: GenesisConfig<S>) -> anyhow::Result<BenchRollup> {
    let sequencer_da_address = genesis_config.sequencer_registry.seq_da_address;
    let prover_address = genesis_config
        .prover_incentives
        .initial_provers
        .first()
        .unwrap()
        .0;

    let rollup_builder = BenchRollupBuilder::new(
        GenesisSource::CustomParams(genesis_config.into_genesis_params()),
        DEFAULT_BLOCK_PRODUCING_CONFIG,
        DEFAULT_FINALIZATION_BLOCKS,
        0,
        mock_da_risc0_host_args(),
    )
    .set_config(|config| {
        config.prover_address = prover_address.to_string();
    })
    .set_da_config(|da_config| {
        da_config.sender_address = sequencer_da_address;
        da_config.block_time_ms = 1_000;
    });

    rollup_builder.start().await
}

/// A simple struct that sends batches to the sequencer on behalf of the user and waits for the results.
pub struct BatchSender {
    nonces: HashMap<<<S as Spec>::CryptoSpec as CryptoSpec>::PublicKey, u64>,
    /// A list of transactions sent we are waiting for inclusion on DA
    txs_to_wait_for: HashSet<HexHash>,
    /// For now we use a slot subscription until we can reliably receive [`TxStatus::Processed`] from the full-node
    slots_subscription: Pin<Box<dyn Stream<Item = Result<Slot, anyhow::Error>> + Send>>,
    /// The client used to send transactions to the sequencer
    client: NodeClient,
}

impl BatchSender {
    /// Creates a new [`BatchSender`].
    pub async fn new(client: NodeClient) -> Self {
        Self {
            nonces: Default::default(),
            txs_to_wait_for: Default::default(),
            slots_subscription: client
                .client
                .subscribe_finalized_slots_with_children(IncludeChildren::new(true))
                .await
                .expect("Impossible to subscribe to the slots"),
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
            "The number of transactions sent should match the number of transactions published by the sequencer"
        );

        for tx_hash in &batch_result.tx_hashes {
            let tx_hash = tx_hash.parse().expect("Invalid tx hash");
            self.txs_to_wait_for.insert(tx_hash);
        }

        Ok(outcomes.into_iter().flatten().collect::<Vec<_>>())
    }

    /// Waits for the results of the transactions sent to the sequencer to be available in the full node.
    pub async fn wait_for_results(&mut self) -> anyhow::Result<()> {
        while !self.txs_to_wait_for.is_empty() {
            let Some(next_slot) = self.slots_subscription.try_next().await? else {
                continue;
            };

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
                    }
                }
            }
        }

        Ok(())
    }
}
