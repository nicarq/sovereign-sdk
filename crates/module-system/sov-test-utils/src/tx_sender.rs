use std::marker::PhantomData;

use async_trait::async_trait;
use sov_api_spec::types;
use sov_cli::NodeClient;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{Runtime, Spec};

/// Submits transactions to the rollup, either directly or via sequencer.
#[async_trait]
pub trait TxSender<S: Spec, R: Runtime<S>> {
    /// Returns the slot number of the batch that was produced.
    async fn send_txs(
        &self,
        client: &NodeClient,
        transactions: &[Transaction<R, S>],
    ) -> anyhow::Result<u64>;
}

/// Submits transactions to the rollup through a sequencer.
#[derive(Default)]
pub struct SequencerTxSender<R: Runtime<S>, S: Spec> {
    phantom: PhantomData<(R, S)>,
}

#[async_trait]
impl<R: Runtime<S>, S: Spec> TxSender<S, R> for SequencerTxSender<R, S> {
    async fn send_txs(
        &self,
        client: &NodeClient,
        transactions: &[Transaction<R, S>],
    ) -> anyhow::Result<u64> {
        let serialized_txs = transactions
            .iter()
            .map(|tx| borsh::to_vec(tx).unwrap())
            .collect::<Vec<_>>();

        let receipt = client.publish_batch(serialized_txs, true).await?;

        let first_tx_hash = receipt
            .tx_hashes
            .first()
            .expect("Tracking the slot number of empty batches in tests is not implemented yet");

        let batch_number = client
            .client
            .get_tx_by_id(
                &types::IntOrHash::Hash(first_tx_hash.parse().unwrap()),
                None,
            )
            .await?
            .data
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Transaction not found"))?
            .batch_number;
        let slot_number = client
            .client
            .get_batch_by_id(&types::IntOrHash::Integer(batch_number), None)
            .await?
            .data
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Batch not found"))?
            .rollup_height;

        Ok(slot_number)
    }
}
