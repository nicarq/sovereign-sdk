#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

use std::hash::Hash;
use std::sync::Arc;

mod batch_builder;
mod mempool;
mod tx_status;
pub mod utils;

pub use batch_builder::FairBatchBuilder;
use jsonrpsee::core::StringError;
use jsonrpsee::types::ErrorObjectOwned;
use jsonrpsee::{PendingSubscriptionSink, RpcModule, SubscriptionMessage};
use serde::{Deserialize, Serialize};
use sov_modules_api::utils::to_jsonrpsee_error_object;
use sov_rollup_interface::da::BlockHeaderTrait;
use sov_rollup_interface::services::batch_builder::{BatchBuilder, TxHash};
use sov_rollup_interface::services::da::DaService;
use tokio::sync::Mutex;
use tracing::info;
use tx_status::TxStatusNotifier;

pub use crate::tx_status::TxStatus;

const SEQUENCER_RPC_ERROR: &str = "SEQUENCER_RPC_ERROR";

/// Single data structure that manages mempool and batch producing.
pub struct Sequencer<B: BatchBuilder, Da: DaService> {
    batch_builder: Mutex<B>,
    da_service: Da,
    tx_status_notifier: Arc<TxStatusNotifier<Da>>,
}

impl<B, Da> Sequencer<B, Da>
where
    B: BatchBuilder + Send + Sync + 'static,
    Da: DaService + Send + Sync + 'static,
    Da::TransactionId: Clone + Send + Sync + serde::Serialize,
{
    /// Creates new Sequencer from BatchBuilder and DaService
    pub fn new(batch_builder: B, da_service: Da) -> Self {
        Self {
            batch_builder: Mutex::new(batch_builder),
            da_service,
            tx_status_notifier: Arc::new(TxStatusNotifier::new()),
        }
    }

    /// Returns the [`jsonrpsee::RpcModule`] for the sequencer-related RPC
    /// methods.
    pub fn rpc(self) -> RpcModule<Self> {
        let mut rpc = RpcModule::new(self);
        Self::register_txs_rpc_methods(&mut rpc).expect("Failed to register sequencer RPC methods");
        rpc
    }

    async fn submit_batch(&self, txs: Vec<Vec<u8>>) -> anyhow::Result<SubmittedBatchInfo> {
        // Acquire the lock before any DA operation, to avoid out-of-order
        // batches and other potential issues.
        let mut batch_builder = self.batch_builder.lock().await;

        let mut accept_tx_results = vec![];
        for tx in txs {
            let result = batch_builder
                .accept_tx(tx.clone())
                .await
                .map(|tx_hash| {
                    self.tx_status_notifier.notify(tx_hash, TxStatus::Submitted);
                    AcceptTxResponse {
                        tx,
                        tx_hash: HexHash(tx_hash),
                    }
                })
                .map_err(|e| to_jsonrpsee_error_object(e, SEQUENCER_RPC_ERROR));
            accept_tx_results.push(result);
        }

        tracing::info!("Submit batch request has been received!");

        let da_height = self
            .da_service
            .get_head_block_header()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch current head: {}", e))?
            .height();

        let blob_txs = batch_builder.get_next_blob(da_height).await?;
        let num_txs = blob_txs.len();
        let (blob, tx_hashes) = blob_txs
            .into_iter()
            .map(|tx| (tx.raw_tx, tx.hash))
            .unzip::<_, _, Vec<_>, Vec<_>>();
        let blob = borsh::to_vec(&blob)?;

        let da_tx_id = match self.da_service.send_transaction(&blob).await {
            Ok(id) => id,
            Err(e) => anyhow::bail!("failed to submit batch: {}", e),
        };

        for tx_hash in tx_hashes {
            self.tx_status_notifier.notify(
                tx_hash,
                TxStatus::Published {
                    da_transaction_id: da_tx_id.clone(),
                },
            );
        }

        Ok(SubmittedBatchInfo { da_height, num_txs })
    }

    async fn accept_tx(&self, tx: Vec<u8>) -> anyhow::Result<TxHash> {
        let mut batch_builder = self.batch_builder.lock().await;

        info!(tx = hex::encode(&tx), "Accepting transaction");
        let tx_hash = batch_builder.accept_tx(tx).await?;
        self.tx_status_notifier.notify(tx_hash, TxStatus::Submitted);

        Ok(tx_hash)
    }

    async fn tx_status(
        &self,
        tx_hash: &TxHash,
    ) -> anyhow::Result<Option<TxStatus<Da::TransactionId>>> {
        let is_in_mempool = self.batch_builder.lock().await.contains(tx_hash).await?;

        if is_in_mempool {
            Ok(Some(TxStatus::Submitted))
        } else {
            Ok(self.tx_status_notifier.get_cached(tx_hash))
        }
    }

    fn register_txs_rpc_methods(rpc: &mut RpcModule<Self>) -> Result<(), jsonrpsee::core::Error> {
        rpc.register_async_method(
            "sequencer_publishBatch",
            |params, batch_builder| async move {
                let mut params_iter = params.sequence();
                let mut txs = vec![];
                while let Some(tx) = params_iter.optional_next()? {
                    txs.push(tx);
                }
                let submitted_batch_info = batch_builder
                    .submit_batch(txs)
                    .await
                    .map_err(|e| to_jsonrpsee_error_object(e, SEQUENCER_RPC_ERROR))?;

                Ok::<SubmittedBatchInfo, ErrorObjectOwned>(submitted_batch_info)
            },
        )?;
        rpc.register_async_method("sequencer_acceptTx", |params, sequencer| async move {
            let tx = params.one::<SubmitTransaction>()?.body;

            sequencer
                .accept_tx(tx.clone())
                .await
                .map(|tx_hash| AcceptTxResponse {
                    tx,
                    tx_hash: HexHash(tx_hash),
                })
                .map_err(|e| to_jsonrpsee_error_object(e, SEQUENCER_RPC_ERROR))
        })?;

        rpc.register_async_method("sequencer_txStatus", |params, sequencer| async move {
            let tx_hash: HexHash = params.one()?;

            let status = sequencer
                .tx_status(&tx_hash.0)
                .await
                .map_err(|e| to_jsonrpsee_error_object(e, SEQUENCER_RPC_ERROR))?;
            Ok::<_, ErrorObjectOwned>(status)
        })?;
        rpc.register_subscription(
            "sequencer_subscribeToTxStatusUpdates",
            "sequencer_newTxStatus",
            "sequencer_unsubscribeToTxStatusUpdates",
            |params, pending, sequencer| async move {
                Self::handle_tx_status_update_subscription(sequencer, params, pending).await
            },
        )?;

        Ok(())
    }

    async fn handle_tx_status_update_subscription(
        sequencer: Arc<Self>,
        params: jsonrpsee::types::Params<'_>,
        sink: PendingSubscriptionSink,
    ) -> Result<(), StringError> {
        let tx_hash: HexHash = params.one()?;
        let mut receiver = sequencer.tx_status_notifier.clone().subscribe(tx_hash.0);

        let subscription = sink.accept().await?;

        let initial_status = sequencer
            .tx_status(&tx_hash.0)
            .await?
            .unwrap_or(TxStatus::Unknown);
        subscription
            .send(SubscriptionMessage::from_json(&initial_status)?)
            .await?;

        while let Ok(new_status) = receiver.recv.recv().await {
            let notification = SubscriptionMessage::from_json(&new_status)?;
            subscription.send(notification).await?;
        }

        Ok(())
    }
}

/// The return type of `sequencer_publishBatch`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmittedBatchInfo {
    /// The DA height for which the batch was submitted.
    pub da_height: u64,
    /// The number of transactions that were successfully included in the batch.
    pub num_txs: usize,
}

/// The response type to the RPC method `sequencer_publishBatch`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishBatchResponse {
    /// Summary information about the batch submission result.
    batch: Result<SubmittedBatchInfo, String>,
    /// Detailed information about the contents of the batch that was submitted
    /// (or attempted to be submitted, if case of an error).
    accept_tx_results: Vec<AcceptTxResponse>,
}

/// The response type to the RPC method `sequencer_acceptTx`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptTxResponse {
    /// Raw transaction contents as originally passed by the client, as a
    /// hex-encoded string.
    #[serde(with = "sov_rollup_interface::rpc::utils::rpc_hex")]
    pub tx: Vec<u8>,
    /// The transaction hash of the transaction that was accepted.
    pub tx_hash: HexHash,
}

/// A 32-byte hash [`serde`]-encoded as a hex string optionally prefixed with
/// `0x`. See [`sov_rollup_interface::rpc::utils::rpc_hex`].
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HexHash(#[serde(with = "sov_rollup_interface::rpc::utils::rpc_hex")] pub TxHash);

/// A transaction to be submitted to the rollup
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SubmitTransaction {
    body: Vec<u8>,
}

impl SubmitTransaction {
    /// Creates a new transaction for submission to the rollup
    pub fn new(body: Vec<u8>) -> Self {
        SubmitTransaction { body }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use sov_mock_da::{MockAddress, MockDaService};
    use sov_rollup_interface::da::BlobReaderTrait;
    use sov_rollup_interface::services::batch_builder::TxWithHash;

    use super::*;

    fn sequencer_rpc(
        batch_builder: MockBatchBuilder,
        da_service: MockDaService,
    ) -> RpcModule<Sequencer<MockBatchBuilder, MockDaService>> {
        Sequencer::new(batch_builder, da_service).rpc()
    }

    /// BatchBuilder used in tests.
    pub struct MockBatchBuilder {
        /// Mempool with transactions.
        pub mempool: Vec<Vec<u8>>,
    }

    // It only takes the first byte of the tx, when submits it.
    // This allows to show effect of batch builder
    #[async_trait]
    impl BatchBuilder for MockBatchBuilder {
        async fn accept_tx(&mut self, tx: Vec<u8>) -> anyhow::Result<TxHash> {
            self.mempool.push(tx);
            Ok([0; 32])
        }

        async fn contains(&self, _tx_hash: &TxHash) -> anyhow::Result<bool> {
            unimplemented!("MockBatchBuilder::contains is not implemented")
        }

        async fn get_next_blob(&mut self, _height: u64) -> anyhow::Result<Vec<TxWithHash>> {
            if self.mempool.is_empty() {
                anyhow::bail!("Mock mempool is empty");
            }
            let txs = std::mem::take(&mut self.mempool)
                .into_iter()
                .filter_map(|tx| {
                    let first_byte = *tx.first()?;
                    Some(TxWithHash {
                        raw_tx: vec![first_byte],
                        hash: [0; 32],
                    })
                })
                .collect();
            Ok(txs)
        }
    }

    #[tokio::test]
    async fn test_submit_on_empty_mempool() {
        let batch_builder = MockBatchBuilder { mempool: vec![] };
        let da_service = MockDaService::new(MockAddress::default());
        let rpc = sequencer_rpc(batch_builder, da_service);

        let arg: &[u8] = &[];
        let result: Result<String, jsonrpsee::core::Error> =
            rpc.call("sequencer_publishBatch", arg).await;

        assert!(result.is_err());
        let error = result.err().unwrap();
        assert_eq!(
            "ErrorObject { code: ServerError(-32001), message: \"SEQUENCER_RPC_ERROR\", data: Some(RawValue(\"Mock mempool is empty\")) }",
            error.to_string()
        );
    }

    #[tokio::test]
    async fn test_submit_happy_path() {
        let tx1 = vec![1, 2, 3];
        let tx2 = vec![3, 4, 5];
        let batch_builder = MockBatchBuilder {
            mempool: vec![tx1.clone(), tx2.clone()],
        };
        let da_service = MockDaService::new(MockAddress::default());
        let rpc = sequencer_rpc(batch_builder, da_service.clone());

        let arg: &[u8] = &[];
        let _: serde_json::Value = rpc.call("sequencer_publishBatch", arg).await.unwrap();

        let mut submitted_block = da_service.get_block_at(1).await.unwrap();
        let block_data = submitted_block.blobs[0].full_data();

        // First bytes of each tx, flattened
        let blob: Vec<Vec<u8>> = vec![vec![tx1[0]], vec![tx2[0]]];
        let expected: Vec<u8> = borsh::to_vec(&blob).unwrap();
        assert_eq!(expected, block_data);
    }

    #[tokio::test]
    async fn test_accept_tx() {
        let batch_builder = MockBatchBuilder { mempool: vec![] };
        let da_service = MockDaService::new(MockAddress::default());

        let rpc = sequencer_rpc(batch_builder, da_service.clone());

        let tx: Vec<u8> = vec![1, 2, 3, 4, 5];
        let request = SubmitTransaction { body: tx.clone() };
        rpc.call::<_, AcceptTxResponse>("sequencer_acceptTx", [request])
            .await
            .unwrap();

        let arg: &[u8] = &[];
        let _: serde_json::Value = rpc.call("sequencer_publishBatch", arg).await.unwrap();

        let mut submitted_block = da_service.get_block_at(1).await.unwrap();
        let block_data = submitted_block.blobs[0].full_data();

        // First bytes of each tx, flattened
        let blob: Vec<Vec<u8>> = vec![vec![tx[0]]];
        let expected: Vec<u8> = borsh::to_vec(&blob).unwrap();
        assert_eq!(expected, block_data);
    }

    #[tokio::test]
    #[ignore = "TBD"]
    async fn test_full_flow() {}
}
