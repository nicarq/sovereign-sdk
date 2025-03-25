mod db;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use bytes::Bytes;
use db::{BlobSenderDb, BlobToSend};
use sov_db::ledger_db::LedgerDb;
use sov_modules_api::{DaSpec, EventModuleName, RuntimeEventResponse};
use sov_rollup_interface::node::da::{DaService, Fee, SubmitBlobReceipt};
use sov_rollup_interface::node::ledger_api::{LedgerStateProvider, QueryMode};
use sov_rollup_interface::node::{future_or_shutdown, FutureOrShutdownOutput};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{debug, error, info, trace};

/// Uniquely identifies a blob managed by the [`BlobSender`].
///
/// Unfortunately, the blob hash can only be known *after*
/// submission to a [`DaService`]. In practice, this means that we need
/// some other way of identifying in-flight blobs short of using the entire blob
/// data as the blob ID (no thanks).
///
/// [`BlobInternalId`] values ought to be generated using UUIDv7s, which
/// makes them strictly monotonically increasing. It's also important to note
/// that [`BlobInternalId`]s ought to be instantiated by
/// callers, rather than [`BlobSender`]. This ensures no loss of data happens in
/// case of a crash at an inconvenient time.
pub type BlobInternalId = u128;

/// See [`BlobInternalId`].
pub fn new_blob_id() -> BlobInternalId {
    uuid::Uuid::now_v7().as_u128()
}

/// Hooks for [`BlobSender`] events.
///
/// We guarantee at-least-once delivery of all events.
#[async_trait]
pub trait BlobSenderHooks: Send + Sync + 'static {
    type Da: DaSpec;

    /// The blob was published and is now part of the DA's canonical chain.
    async fn on_published_blob(
        &self,
        _blob_id: BlobInternalId,
        _blob_hash: [u8; 32],
        _da_tx_id: <Self::Da as DaSpec>::TransactionId,
    ) {
    }

    /// The blob was processed by the rollup node, but it may not be finalized yet.
    async fn on_processed_blob(
        &self,
        _blob_id: BlobInternalId,
        _blob_hash: [u8; 32],
        _da_tx_id: <Self::Da as DaSpec>::TransactionId,
    ) {
    }

    /// The blob is considered to be finalized by the rollup node.
    async fn on_finalized_blob(
        &self,
        _blob_id: BlobInternalId,
        _blob_hash: [u8; 32],
        _da_tx_id: <Self::Da as DaSpec>::TransactionId,
    ) {
    }
}

/// A reusable component that manages blob submission to the [`DaService`].
#[derive(Clone)]
pub struct BlobSender<Da: DaService, H> {
    blob_sender: mpsc::UnboundedSender<BlobSubmissionRequest<Da>>,
    db: Arc<BlobSenderDb>,
    hooks: Arc<H>,
    last_blob_id: Option<BlobInternalId>,
}

impl<Da, H> BlobSender<Da, H>
where
    Da: DaService,
    H: BlobSenderHooks<Da = Da::Spec>,
{
    pub async fn new(
        da: Da,
        ledger_db: LedgerDb,
        storage_path: &Path,
        // TODO(@neysofu): all blobs are sent in parallel as of now.
        _parallel_submission: bool,
        hooks: H,
        shutdown_receiver: watch::Receiver<()>,
    ) -> anyhow::Result<(Self, JoinHandle<()>)> {
        let db = Arc::new(BlobSenderDb::new(storage_path).await?);
        let (blob_sender, blob_receiver) = mpsc::unbounded_channel();

        let hooks = Arc::new(hooks);

        let task_state = TaskState {
            inner: Arc::new(TaskStateInner {
                da,
                ledger_db,
                db: db.clone(),
                hooks: hooks.clone(),
                shutdown_receiver,
            }),
        };

        let all_blobs = db.get_all().await?;
        let last_blob_id = all_blobs.last().map(|b| b.blob_id);

        let mut sender = Self {
            blob_sender,
            db,
            hooks,
            last_blob_id,
        };

        let handle = tokio::spawn(async move {
            task_state.main_background_task(blob_receiver).await;
        });

        for b in all_blobs {
            sender
                .publish_blob_inner(b.blob, b.blob_id, b.latest_known_processing_state)
                .await?;
        }

        Ok((sender, handle))
    }

    /// Returns a reference to the [`BlobSenderHooks`] instance.
    pub fn hooks(&self) -> &H {
        &self.hooks
    }

    /// Can be called again with the same [`BlobInternalId`] to resume publishing.
    pub async fn publish_batch_blob(
        &mut self,
        data: Bytes,
        id: BlobInternalId,
    ) -> anyhow::Result<()> {
        self.publish_blob_inner(
            BlobToSend::Batch { data },
            id,
            BlobProcessingState::MustSubmit,
        )
        .await
    }

    /// Can be called again with the same [`BlobInternalId`] to resume publishing.
    pub async fn publish_proof_blob(
        &mut self,
        data: Bytes,
        id: BlobInternalId,
    ) -> anyhow::Result<()> {
        self.publish_blob_inner(
            BlobToSend::Proof { data },
            id,
            BlobProcessingState::MustSubmit,
        )
        .await
    }

    async fn publish_blob_inner(
        &mut self,
        blob: BlobToSend,
        blob_id: BlobInternalId,
        latest_known_processing_state: BlobProcessingState<Da>,
    ) -> anyhow::Result<()> {
        if let Some(last_blob_id) = self.last_blob_id {
            if blob_id <= last_blob_id {
                info!(
                    blob_id,
                    last_blob_id,
                    "No need to publish blob as it's already in-flight or awaiting finalization. Skipping."
                );
                return Ok(());
            }
        }

        self.db.push(blob.clone(), blob_id).await?;
        self.blob_sender.send(BlobSubmissionRequest {
            blob,
            blob_id,
            latest_known_processing_state,
        })?;
        self.last_blob_id = Some(blob_id);

        Ok(())
    }
}

type BlobReceiptFut<Da> = oneshot::Receiver<
    Result<
        SubmitBlobReceipt<<<Da as DaService>::Spec as DaSpec>::TransactionId>,
        <Da as DaService>::Error,
    >,
>;

struct TaskStateInner<Da: DaService> {
    da: Da,
    ledger_db: LedgerDb,
    db: Arc<BlobSenderDb>,
    hooks: Arc<dyn BlobSenderHooks<Da = Da::Spec>>,
    shutdown_receiver: watch::Receiver<()>,
}

#[derive(Clone)]
struct TaskState<Da: DaService> {
    inner: Arc<TaskStateInner<Da>>,
}

impl<Da: DaService> std::ops::Deref for TaskState<Da> {
    type Target = TaskStateInner<Da>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<Da: DaService> TaskState<Da> {
    const RESUBMIT_INTERVAL: Duration = Duration::from_secs(20);
    const LEDGER_POLL_INTERVAL: Duration = Duration::from_secs(1);

    #[tracing::instrument(skip_all, level = "debug")]
    async fn main_background_task(
        &self,
        mut blob_receiver: mpsc::UnboundedReceiver<BlobSubmissionRequest<Da>>,
    ) {
        let mut handles = vec![];

        loop {
            let fut = future_or_shutdown(blob_receiver.recv(), &self.shutdown_receiver);
            let FutureOrShutdownOutput::Output(channel_msg) = fut.await else {
                debug!("Received a shutdown signal");
                break;
            };

            let Some(BlobSubmissionRequest {
                blob,
                blob_id,
                latest_known_processing_state,
            }) = channel_msg
            else {
                // Channel was closed, the node is shutting down.
                break;
            };

            let shutdown_receiver = self.shutdown_receiver.clone();
            handles.push(tokio::task::spawn({
                let state = self.clone();

                async move {
                    let fut = state.manage_blob_submission_inside_task(
                        blob,
                        blob_id,
                        latest_known_processing_state,
                    );
                    let res = future_or_shutdown(fut, &shutdown_receiver).await;

                    match res {
                        FutureOrShutdownOutput::Output(Ok(())) |
                            FutureOrShutdownOutput::Shutdown => {},
                        FutureOrShutdownOutput::Output(Err(err)) => {
                            error!(%err, %blob_id, "Error while submitting blob; this is either a bug or a database issue");
                        },
                    }
                }
            }));

            // Clean up finished tasks.
            handles.retain(|handle| !handle.is_finished());
        }

        debug!(
            num_handles_to_join = handles.len(),
            "Exiting the blob sender background task..."
        );

        for handle in handles {
            if let Err(err) = handle.await {
                error!(%err, "Error in a blob sender background task");
            }
        }

        debug!("Blob sender background task exited");
    }

    #[tracing::instrument(skip(self, blob), level = "debug")]
    async fn manage_blob_submission_inside_task(
        &self,
        blob: BlobToSend,
        blob_id: BlobInternalId,
        latest_known_processing_state: BlobProcessingState<Da>,
    ) -> anyhow::Result<()> {
        let mut blob_state = latest_known_processing_state;

        'outer: loop {
            trace!(?blob_state, ?blob_id, "Tracking blob submission state");

            match blob_state {
                BlobProcessingState::MustSubmit => {
                    let receipt_fut = self.send_blob(blob.clone()).await?;

                    self.db.set_state(blob_id, &blob_state).await?;

                    tokio::select! {
                        receipt_res = receipt_fut => {
                            let receipt = receipt_res?.map_err(|err| anyhow::anyhow!("Failed to track blob submission: {err}"))?;
                            blob_state = BlobProcessingState::Published { receipt };
                        }
                        _ = sleep(Self::RESUBMIT_INTERVAL) => {
                            // We successfully submitted the blob, but it wasn't
                            // published despite waiting for quite some time.
                            // Possibly the fee was too low. Let's try again.

                            trace!(?blob_state, "Blob submission timed out, retrying");
                            blob_state = BlobProcessingState::MustSubmit;
                        },
                    };
                }
                BlobProcessingState::Published { receipt } => {
                    self.hooks
                        .on_published_blob(
                            blob_id,
                            receipt.blob_hash.into(),
                            receipt.da_transaction_id.clone(),
                        )
                        .await;
                    self.db
                        .set_state(
                            blob_id,
                            &BlobProcessingState::<Da>::Published {
                                receipt: receipt.clone(),
                            },
                        )
                        .await?;

                    let timer = SystemTime::now();
                    loop {
                        if timer.elapsed()? > Self::RESUBMIT_INTERVAL {
                            debug!(
                                blob_id,
                                blob_hash = %receipt.blob_hash,
                                resubmit_interval_secs = %Self::RESUBMIT_INTERVAL.as_secs(),
                                "Published blob was not processed by the rollup node despite waiting for quite some time. Re-submitting"
                            );
                            blob_state = BlobProcessingState::MustSubmit;
                            continue 'outer;
                        }

                        let finality_status =
                            self.is_blob_finalized(receipt.blob_hash.into()).await?;

                        match finality_status {
                            Some(_) => {
                                // Never skip directly to `Finalized` state, or
                                // we won't send out the notification.
                                blob_state = BlobProcessingState::Processed { receipt };
                                break;
                            }
                            None => {
                                sleep(Duration::from_secs(1)).await;
                            }
                        }
                    }
                }
                BlobProcessingState::Processed { receipt } => {
                    self.hooks
                        .on_processed_blob(
                            blob_id,
                            receipt.blob_hash.into(),
                            receipt.da_transaction_id.clone(),
                        )
                        .await;

                    self.db
                        .set_state(
                            blob_id,
                            &BlobProcessingState::<Da>::Processed {
                                receipt: receipt.clone(),
                            },
                        )
                        .await?;

                    loop {
                        let finality_status =
                            self.is_blob_finalized(receipt.blob_hash.into()).await?;

                        match finality_status {
                            Some(false) => {
                                sleep(Self::LEDGER_POLL_INTERVAL).await;
                                continue;
                            }
                            Some(true) => {
                                blob_state = BlobProcessingState::Finalized { receipt };
                                break;
                            }
                            None => {
                                debug!(
                                    blob_id,
                                    blob_hash = %receipt.blob_hash,
                                    "Re-org detected; resubmitting blob"
                                );
                                blob_state = BlobProcessingState::MustSubmit;
                                break;
                            }
                        }
                    }
                }
                BlobProcessingState::Finalized { receipt } => {
                    // Upon crashing, we'd rather call the hook twice rather than not
                    // calling it at all. So, we call it *before* removing the blob from
                    // the database.
                    self.hooks
                        .on_finalized_blob(
                            blob_id,
                            receipt.blob_hash.into(),
                            receipt.da_transaction_id,
                        )
                        .await;
                    self.db.remove(blob_id).await?;

                    break;
                }
            }
        }

        trace!("Exiting blob submission task");

        Ok(())
    }

    async fn is_blob_finalized(&self, blob_hash: [u8; 32]) -> anyhow::Result<Option<bool>> {
        let Some(batch) = self
            .ledger_db
            .get_batch_by_hash::<(), (), RuntimeEventResponse<IgnoreEvent>>(
                &blob_hash,
                QueryMode::Compact,
            )
            .await?
        else {
            return Ok(None);
        };

        let slot_number = batch.rollup_height;
        let latest_finalized_slot_number =
            self.ledger_db.get_latest_finalized_slot_number().await?;

        Ok(Some(slot_number >= latest_finalized_slot_number))
    }

    async fn send_blob(&self, blob: BlobToSend) -> anyhow::Result<BlobReceiptFut<Da>> {
        let fee = self
            .da
            .estimate_fee(blob.data().len())
            .await
            .map_err(|da_err| anyhow::anyhow!("Failed to estimate fee: {da_err}"))?;

        trace!(
            gas_estimate = fee.gas_estimate(),
            "Will attempt to publish batch to DA"
        );

        Ok(self.da.send_transaction(blob.data(), fee).await)
    }
}

struct BlobSubmissionRequest<Da: DaService> {
    blob: BlobToSend,
    blob_id: BlobInternalId,
    latest_known_processing_state: BlobProcessingState<Da>,
}

#[derive(derive_more::Debug, Clone, serde::Serialize, serde::Deserialize)]
#[debug(bounds())]
enum BlobProcessingState<Da: DaService> {
    MustSubmit,
    Published {
        receipt: SubmitBlobReceipt<<Da::Spec as DaSpec>::TransactionId>,
    },
    Processed {
        receipt: SubmitBlobReceipt<<Da::Spec as DaSpec>::TransactionId>,
    },
    Finalized {
        receipt: SubmitBlobReceipt<<Da::Spec as DaSpec>::TransactionId>,
    },
}

/// We use it as a [`RuntimeEventResponse`] generic when we don't care about event data.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
struct IgnoreEvent;

impl EventModuleName for IgnoreEvent {
    fn module_name(&self) -> &'static str {
        "ignore"
    }
}
