use std::marker::PhantomData;
use std::path::Path;
use std::sync::Arc;

use rockbound::gen_rocksdb_options;
use sov_rollup_interface::node::da::{DaService, Fee, SubmitBlobReceipt};
use sov_rollup_interface::node::{future_or_shutdown, FutureOrShutdownOutput};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, trace};

use crate::common::WithCachedTxHashes;
use crate::{BlobReceiptFut, SubmitBatchReceipt, TxStatus, TxStatusManager};

type DbBlobId = u128;

/// A reusable component that manages blob submission to the [`DaService`].
// TODO(@neysofu):
//  1. Backpressure.
//  2. Sending proofs.
//  3. Ideally, unify all background tasks into one, with shared context and
//     proper graceful shutdown.
pub struct BlobSender<Da: DaService, Batch> {
    da: Da,
    db: BlobSenderDb,
    txsm: TxStatusManager<Da::Spec>,
    phantom: PhantomData<Batch>,
    shutdown_receiver: watch::Receiver<()>,
}

impl<Da, Batch> BlobSender<Da, Batch>
where
    Da: DaService,
    Batch: borsh::BorshSerialize + borsh::BorshDeserialize,
{
    pub async fn new(
        da: Da,
        storage_path: &Path,
        txsm: TxStatusManager<Da::Spec>,
        parallel_submission: bool,
        shutdown_receiver: watch::Receiver<()>,
    ) -> anyhow::Result<Self> {
        let db = BlobSenderDb::new(storage_path).await?;

        let sender = Self {
            da,
            db,
            txsm,
            phantom: PhantomData,
            shutdown_receiver,
        };

        // Resume sending all pending batches.
        for blob in sender.db.get_all().await? {
            match blob {
                BlobToSend::Batch(batch) => {
                    let batch = WithCachedTxHashes {
                        inner: borsh::from_slice(&batch.inner)?,
                        tx_hashes: batch.tx_hashes,
                    };

                    if parallel_submission {
                        sender.publish_batch(batch).await?;
                    } else {
                        sender.publish_batch_and_wait(batch).await?;
                    }
                }
                BlobToSend::Proof(_blob) => {
                    // TODO(@neysofu).
                }
            }
        }

        Ok(sender)
    }

    pub async fn publish_batch_and_wait(
        &self,
        batch: WithCachedTxHashes<Batch>,
    ) -> anyhow::Result<SubmitBatchReceipt> {
        self.publish_batch(batch)
            .await?
            .await
            .expect("Failed to .await a task; this is a bug, please report it")
    }

    pub async fn publish_batch(
        &self,
        batch: WithCachedTxHashes<Batch>,
    ) -> anyhow::Result<JoinHandle<anyhow::Result<SubmitBatchReceipt>>> {
        let db_batch = WithCachedTxHashes {
            tx_hashes: batch.tx_hashes.clone(),
            inner: borsh::to_vec(&batch.inner)?,
        };
        let blob_id = self.db.push_batch(db_batch).await?;

        let receipt_fut = send_batch(&self.da, batch, blob_id, &self.db).await?;

        let txsm = self.txsm.clone();
        let shutdown_receiver = self.shutdown_receiver.clone();

        Ok(tokio::spawn(async move {
            let fut = react_to_batch_receipt::<Da>(receipt_fut, &txsm);
            match future_or_shutdown(fut, &shutdown_receiver).await {
                FutureOrShutdownOutput::Shutdown => anyhow::bail!("Shutting down"),
                FutureOrShutdownOutput::Output(res) => res,
            }
        }))
    }
}

async fn send_batch<Da, Batch>(
    da: &Da,
    batch: WithCachedTxHashes<Batch>,
    blob_id: DbBlobId,
    db: &BlobSenderDb,
) -> anyhow::Result<WithCachedTxHashes<BlobReceiptFut<Da>>>
where
    Da: DaService,
    Batch: borsh::BorshSerialize,
{
    let WithCachedTxHashes {
        inner: next_batch,
        tx_hashes,
    } = batch;

    let serialized_batch = borsh::to_vec(&next_batch)
        .expect("Failed to serialize batch inside sequencer; this is a bug, please report it");

    let fee = match da.estimate_fee(serialized_batch.len()).await {
        Ok(fee) => fee,
        Err(e) => anyhow::bail!(
            "failed to submit batch: could not determine appropriate fee rate: {}",
            e
        ),
    };

    trace!(
        gas_estimate = fee.gas_estimate(),
        txs_count = tx_hashes.len(),
        "Will attempt to publish batch to DA"
    );

    let receipt_fut = da.send_transaction(&serialized_batch, fee).await;

    // If we crash here, the batch will still be sitting inside the
    // database and it will be re-submitted once again upon node restart.
    //
    // Not ideal, but certainly better than losing it forever. This is the
    // correct behavior.

    db.remove(blob_id).await?;

    Ok(WithCachedTxHashes {
        inner: receipt_fut,
        tx_hashes,
    })
}

async fn react_to_batch_receipt<Da: DaService>(
    receipt_fut: WithCachedTxHashes<BlobReceiptFut<Da>>,
    txsm: &TxStatusManager<Da::Spec>,
) -> anyhow::Result<SubmitBatchReceipt> {
    let receipt = receipt_fut
        .inner
        .await
        .expect("Failed to .await a oneshot receiver; this is a bug, please report it")
        .map_err(|e| anyhow::anyhow!("Failed to provide batch submission receipt: {e}"))?;

    let SubmitBlobReceipt {
        blob_hash,
        da_transaction_id,
    } = &receipt;

    debug!(%da_transaction_id, %blob_hash, "Batch has been sent");

    for tx_hash in &receipt_fut.tx_hashes {
        txsm.notify(
            *tx_hash,
            TxStatus::Published {
                da_tx_id: receipt.da_transaction_id.clone(),
            },
        );
    }

    Ok(SubmitBatchReceipt {
        tx_hashes: receipt_fut.tx_hashes,
    })
}

// Private!!! Not part of the API, and we'd like to keep it that way.
#[derive(Debug)]
struct BlobSenderDb {
    db: Arc<rockbound::DB>,
}

impl BlobSenderDb {
    const DB_NAME: &'static str = "blob_sender";
    const TABLES: &'static [&'static str] = &[tables::BlobsToSend::table_name()];

    async fn new(path: &Path) -> anyhow::Result<Self> {
        let db = Arc::new(rockbound::DB::open(
            path.join(Self::DB_NAME),
            Self::DB_NAME,
            Self::TABLES.iter().copied(),
            &gen_rocksdb_options(&Default::default(), false),
        )?);

        Ok(Self { db })
    }

    async fn get_all(&self) -> anyhow::Result<Vec<BlobToSend>> {
        let mut blobs = vec![];

        for iter_res in self.db.iter::<tables::BlobsToSend>()? {
            let item = iter_res?;
            blobs.push(item.value);
        }

        Ok(blobs)
    }

    async fn push_batch(&self, batch: WithCachedTxHashes<Vec<u8>>) -> anyhow::Result<DbBlobId> {
        self.push_internal(BlobToSend::Batch(batch)).await
    }

    async fn push_internal(&self, blob: BlobToSend) -> anyhow::Result<DbBlobId> {
        let id = uuid::Uuid::now_v7().as_u128();
        self.db.put_async::<tables::BlobsToSend>(&id, &blob).await?;

        Ok(id)
    }

    async fn remove(&self, id: DbBlobId) -> anyhow::Result<()> {
        self.db.delete::<tables::BlobsToSend>(&id)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, borsh::BorshSerialize, borsh::BorshDeserialize)]
pub enum BlobToSend {
    Batch(WithCachedTxHashes<Vec<u8>>),
    Proof(Vec<u8>),
}

pub mod tables {
    use sov_db::{
        define_table_with_seek_key_codec, define_table_without_codec, impl_borsh_value_codec,
    };

    use super::*;

    define_table_with_seek_key_codec!(
        (BlobsToSend) DbBlobId => BlobToSend
    );
}
