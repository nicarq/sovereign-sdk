use std::marker::PhantomData;
use std::sync::Arc;

use futures::StreamExt;
use serde::de::DeserializeOwned;
use sov_db::ledger_db::LedgerDb;
use sov_modules_api::capabilities::Authenticator;
use sov_modules_api::{Batch, RawTx, TxReceiptContents};
use sov_rollup_interface::da::{BlockHeaderTrait, DaBlobHash};
use sov_rollup_interface::rpc::{ItemOrHash, LedgerStateProvider, QueryMode};
use sov_rollup_interface::services::batch_builder::{AcceptTxError, BatchBuilder, TxWithHash};
use sov_rollup_interface::services::da::DaService;
use sov_rollup_interface::TxHash;
use tokio::sync::{oneshot, Mutex};
use tracing::info;

use super::tx_status::{TxStatus, TxStatusManager};
use super::SubmittedBatchInfo;

/// A bunch of associated types that define the behavior of a [`Sequencer`].
pub trait SequencerSpec: Clone + Send + Sync + 'static {
    /// The [`BatchBuilder`] that the sequencer uses to process submitted
    /// transactions and assemble them into batches.
    type BatchBuilder: BatchBuilder;
    /// The [`DaService`] that the sequencer uses to communicate with the DA
    /// layer.
    type Da: DaService;
    /// The type of the batch receipt that the rollup stores in the [`LedgerDb`].
    type BatchReceipt: DeserializeOwned + Send + Sync;
    /// The type of the transaction receipt that the rollup stores in the
    /// [`LedgerDb`].
    type TxReceipt: TxReceiptContents;
}

/// A [`SequencerSpec`] with explicit generic types.
#[derive(derivative::Derivative)]
#[derivative(Clone(bound = ""))]
pub struct GenericSequencerSpec<B, Da, Auth, BatchReceipt, TxReceipt>(
    PhantomData<(B, Da, Auth, BatchReceipt, TxReceipt)>,
);

impl<B, Da, Auth, BatchReceipt, TxReceipt> SequencerSpec
    for GenericSequencerSpec<B, Da, Auth, BatchReceipt, TxReceipt>
where
    B: BatchBuilder,
    Da: DaService,
    Auth: Authenticator,
    BatchReceipt: DeserializeOwned + Send + Sync + 'static,
    TxReceipt: TxReceiptContents,
{
    type BatchBuilder = B;
    type Da = Da;
    type BatchReceipt = BatchReceipt;
    type TxReceipt = TxReceipt;
}

/// Single data structure that manages mempool and batch producing.
#[derive(Clone)]
pub struct Sequencer<Ss: SequencerSpec>(Arc<Inner<Ss>>);

struct Inner<Ss: SequencerSpec> {
    batch_builder: Mutex<Ss::BatchBuilder>,
    da_service: Ss::Da,
    tx_status_manager: TxStatusManager<<Ss::Da as DaService>::Spec>,
    dropper: Option<oneshot::Sender<()>>,
}

impl<Ss: SequencerSpec> Drop for Inner<Ss> {
    fn drop(&mut self) {
        // `send` takes ownership over the sender, but we can't do that within
        // `drop`. So we use `Option::take` as a hack instead.
        self.dropper.take().map(|dropper| dropper.send(()));
    }
}

impl<Ss: SequencerSpec> Sequencer<Ss> {
    /// Creates a new [`Sequencer`] from a [`BatchBuilder`] and a [`DaService`].
    pub fn new(
        batch_builder: Ss::BatchBuilder,
        da_service: Ss::Da,
        tx_status_manager: TxStatusManager<<Ss::Da as DaService>::Spec>,
        ledger_db: LedgerDb,
    ) -> Self {
        let (dropper, drop_receiver) = oneshot::channel();

        tokio::spawn({
            let txsm = tx_status_manager.clone();
            async move {
                listen_for_slots_and_notify::<Ss>(ledger_db, txsm, drop_receiver)
                    .await
                    .ok();
            }
        });

        Self(Arc::new(Inner {
            batch_builder: Mutex::new(batch_builder),
            da_service,
            tx_status_manager,
            dropper: Some(dropper),
        }))
    }

    pub(crate) fn tx_status_manager(&self) -> &TxStatusManager<<Ss::Da as DaService>::Spec> {
        &self.0.tx_status_manager
    }

    /// Calls [`BatchBuilder::accept_tx`] for each transaction, and finally
    /// [`BatchBuilder::get_next_blob`].
    pub async fn submit_batch(&self, txs: Vec<Vec<u8>>) -> anyhow::Result<SubmittedBatchInfo> {
        // Acquire the lock before any DA operation, to avoid out-of-order
        // batches and other potential issues.
        let mut batch_builder = self.0.batch_builder.lock().await;

        let mut accept_tx_results = vec![];
        for tx in txs {
            let result = batch_builder
                .accept_tx(tx.clone())
                .await
                .map(|tx_with_hash| {
                    // Send notification.
                    self.0
                        .tx_status_manager
                        .notify(tx_with_hash.hash, TxStatus::Submitted);
                });

            accept_tx_results.push(result);
        }

        tracing::info!("Submit batch request has been received!");

        let da_height = self
            .0
            .da_service
            .get_head_block_header()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch current head: {}", e))?
            .height();

        let blob_txs = batch_builder.get_next_blob(da_height).await?;
        let num_txs = blob_txs.len();

        let mut txs = Vec::with_capacity(num_txs);
        let mut tx_hashes = Vec::with_capacity(num_txs);

        for tx in blob_txs {
            txs.push(RawTx { data: tx.raw_tx });
            tx_hashes.push(tx.hash);
        }

        let batch = Batch { txs };
        let serialized_batch = borsh::to_vec(&batch)?;

        let fee = match self.0.da_service.estimate_fee(serialized_batch.len()).await {
            Ok(fee) => fee,
            Err(e) => anyhow::bail!(
                "failed to submit batch: could not determine appropriate fee rate: {}",
                e
            ),
        };
        let da_tx_id = match self
            .0
            .da_service
            .send_transaction(&serialized_batch, fee)
            .await
        {
            Ok(id) => id,
            Err(e) => anyhow::bail!("failed to submit batch: {}", e),
        };

        for tx_hash in tx_hashes {
            self.0.tx_status_manager.notify(
                tx_hash,
                TxStatus::Published {
                    da_transaction_id: da_tx_id.clone(),
                },
            );
        }

        Ok(SubmittedBatchInfo { da_height, num_txs })
    }

    /// See [`BatchBuilder::accept_tx`].
    pub async fn accept_tx(&self, tx: Vec<u8>) -> Result<TxWithHash, AcceptTxError> {
        let mut batch_builder = self.0.batch_builder.lock().await;

        tracing::info!(tx = hex::encode(&tx), "Accepting transaction");
        let tx_with_hash = batch_builder.accept_tx(tx).await?;
        self.0
            .tx_status_manager
            .notify(tx_with_hash.hash, TxStatus::Submitted);

        Ok(tx_with_hash)
    }

    /// Queries the latest known status of the given transaction. Best-effort,
    /// can't promise to always know the status.
    pub async fn tx_status(
        &self,
        tx_hash: &TxHash,
    ) -> anyhow::Result<Option<TxStatus<DaBlobHash<<Ss::Da as DaService>::Spec>>>> {
        let is_in_mempool = self.0.batch_builder.lock().await.contains(tx_hash).await?;

        if is_in_mempool {
            Ok(Some(TxStatus::Submitted))
        } else {
            Ok(self.0.tx_status_manager.get_cached(tx_hash))
        }
    }
}

async fn notify_processed_slot<Ss: SequencerSpec>(
    ledger_db: &LedgerDb,
    tx_status_manager: TxStatusManager<<Ss::Da as DaService>::Spec>,
    slot_number: u64,
) -> anyhow::Result<()> {
    let slot = ledger_db
        .get_slot_by_number::<Ss::BatchReceipt, Ss::TxReceipt>(slot_number, QueryMode::Full)
        .await?;
    for batch in slot.unwrap().batches.unwrap_or_default().iter() {
        let ItemOrHash::Full(batch) = batch else {
            continue;
        };
        for tx in batch.txs.as_deref().unwrap_or_default().iter() {
            let ItemOrHash::Full(tx) = tx else {
                continue;
            };

            let da_transaction_id =
                <DaBlobHash<<Ss::Da as DaService>::Spec>>::try_from(batch.hash)?;
            let tx_hash = TxHash::new(tx.hash);

            tx_status_manager.notify(tx_hash, TxStatus::Published { da_transaction_id });
        }
    }

    Ok(())
}

pub async fn listen_for_slots_and_notify<Ss: SequencerSpec>(
    ledger_db: LedgerDb,
    tx_status_manager: TxStatusManager<<Ss::Da as DaService>::Spec>,
    mut drop_receiver: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let mut sub = ledger_db.subscribe_slots();

    loop {
        tokio::select! {
            _ = &mut drop_receiver => {
                info!("Sequencer was dropped, stopping listener for new slots");
                break;
            },
            slot_number_opt = sub.next() => {
                if let Some(slot_number) = slot_number_opt {
                    notify_processed_slot::<Ss>(&ledger_db, tx_status_manager.clone(), slot_number).await?;
                } else {
                    break;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use async_trait::async_trait;
    use base64::prelude::*;
    use borsh::BorshDeserialize;
    use sov_mock_da::{MockAddress, MockDaService};
    use sov_modules_api::prelude::*;
    use sov_rollup_interface::da::BlobReaderTrait;
    use sov_rollup_interface::services::batch_builder::TxWithHash;
    use sov_sequencer_json_client::types;
    use sov_test_utils::sequencer::TestSequencerSetup;

    use super::*;

    async fn new_sequencer(
        batch_builder: MockBatchBuilder,
    ) -> TestSequencerSetup<MockBatchBuilder> {
        let dir = tempfile::tempdir().unwrap();
        let da_service = MockDaService::new(MockAddress::default());

        TestSequencerSetup::new(dir, da_service, batch_builder, Default::default())
            .await
            .unwrap()
    }

    /// BatchBuilder used in tests.
    #[derive(Default)]
    pub struct MockBatchBuilder {
        /// Mempool with transactions.
        pub mempool: Vec<Vec<u8>>,
    }

    // It only takes the first byte of the tx, when submits it.
    // This allows to show an effect of batch builder
    #[async_trait]
    impl BatchBuilder for MockBatchBuilder {
        async fn accept_tx(&mut self, tx: Vec<u8>) -> Result<TxWithHash, AcceptTxError> {
            self.mempool.push(tx.clone());
            Ok(TxWithHash {
                raw_tx: tx,
                hash: TxHash::new([0; 32]),
            })
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
                .map(|raw_tx| TxWithHash {
                    raw_tx,
                    hash: TxHash::new([0; 32]),
                })
                .collect();
            Ok(txs)
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn dropping_sequencer_stops_listener() {
        let sequencer = new_sequencer(MockBatchBuilder::default()).await;

        assert!(!logs_contain("stopping listener"));

        drop(sequencer);
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(logs_contain("stopping listener"));
    }

    #[tokio::test]
    async fn test_submit_on_empty_mempool() {
        let sequencer = new_sequencer(MockBatchBuilder::default()).await;
        let client = sequencer.client();

        let error_response = client
            .publish_batch(&types::PublishBatchBody {
                transactions: vec![],
            })
            .await
            .unwrap_err();

        dbg!(&error_response);
        assert_eq!(error_response.status().map(|s| s.as_u16()), Some(409));
    }

    #[tokio::test]
    async fn test_submit_happy_path() {
        let tx1 = vec![1, 2, 3];
        let tx2 = vec![3, 4, 5];

        let batch_builder = MockBatchBuilder {
            mempool: vec![tx1.clone(), tx2.clone()],
        };
        let sequencer = new_sequencer(batch_builder).await;

        sequencer
            .client()
            .publish_batch(&types::PublishBatchBody {
                transactions: vec![],
            })
            .await
            .unwrap();

        let mut submitted_block = sequencer.da_service.get_block_at(1).await.unwrap();
        let block_data = submitted_block.batch_blobs[0].full_data();

        let batch = Batch::try_from_slice(block_data).unwrap();

        assert_eq!(batch.txs.len(), 2);
        assert_eq!(tx1, batch.txs[0].data);
        assert_eq!(tx2, batch.txs[1].data);
    }

    #[tokio::test]
    async fn test_accept_tx() {
        let batch_builder = MockBatchBuilder { mempool: vec![] };
        let da_service = MockDaService::new(MockAddress::default());

        let sequencer = TestSequencerSetup::new(
            tempfile::tempdir().unwrap(),
            da_service.clone(),
            batch_builder,
            Default::default(),
        )
        .await
        .unwrap();

        let client = sequencer.client();

        let tx: Vec<u8> = vec![1, 2, 3, 4, 5];

        client
            .accept_tx(&types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
            .unwrap();

        client
            .publish_batch(&types::PublishBatchBody {
                transactions: vec![],
            })
            .await
            .unwrap();

        let mut submitted_block = da_service.get_block_at(1).await.unwrap();
        let block_data = submitted_block.batch_blobs[0].full_data();

        let batch = Batch::try_from_slice(block_data).unwrap();

        assert_eq!(tx, batch.txs[0].data);
    }
}
