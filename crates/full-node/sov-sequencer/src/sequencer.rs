use std::path::Path;
use std::sync::Arc;

use axum::async_trait;
use sov_db::ledger_db::LedgerDb;
use sov_modules_api::rest::utils::ErrorObject;
use sov_modules_api::rest::{ApiState, StateUpdateReceiver};
use sov_modules_api::{
    DaSyncState, FullyBakedTx, RawTx, RuntimeEventResponse, Spec, StateUpdateInfo,
};
use sov_rollup_interface::common::SlotNumber;
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::node::da::{DaService, Fee, SubmitBlobReceipt};
use sov_rollup_interface::node::ledger_api::{
    ItemOrHash, LedgerStateProvider, QueryMode, SlotResponse,
};
use sov_rollup_interface::node::{future_or_shutdown, FutureOrShutdownOutput};
use sov_rollup_interface::TxHash;
use tokio::sync::{broadcast, oneshot, Mutex, MutexGuard};
use tracing::{debug, error, info, trace};

use super::tx_status::TxStatus;
use crate::batch_builders::{AcceptedTx, BatchBuilder, SequencerConfirmation, WithCachedTxHashes};
use crate::{SequencerConfig, SequencerSpec, SubmitBatchReceipt, TxStatusManager};

/// Single data structure that manages mempool and batch producing.
#[derive(Clone, derive_more::Deref)]
pub struct Sequencer<Ss: SequencerSpec> {
    // Makes it cheaply clonable.
    inner: Arc<Inner<Ss>>,
}

pub struct Inner<Ss: SequencerSpec> {
    batch_builder: Mutex<Ss::BatchBuilder>,
    // The sequencer's own copy of the batch-builder's API state. This is
    // automatically updated by the batch builder with the latest state.
    // We simply cache a copy so that we don't need to lock the builder to
    // retrieve it when needed.
    api_state: ApiState<<Ss::BatchBuilder as BatchBuilder>::Spec>,
    events_sender: broadcast::Sender<SequencerEvent<Ss::BatchBuilder>>,
    da_service: Ss::Da,
    tx_status_manager: TxStatusManager<<Ss::Da as DaService>::Spec>,
}

impl<Ss: SequencerSpec> Sequencer<Ss> {
    // FIXME(@neysofu): this is way too small.
    const EVENTS_CHANNEL_SIZE: usize = 100;

    /// Creates a new [`Sequencer`] from a [`BatchBuilder`] and a [`DaService`].
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    pub async fn new(
        state_update_receiver: StateUpdateReceiver<
            <<Ss::BatchBuilder as BatchBuilder>::Spec as Spec>::Storage,
        >,
        da_service: Ss::Da,
        da_sync_state: Arc<DaSyncState>,
        storage_path: &Path,
        ledger_db: LedgerDb,
        config: &SequencerConfig<
            <<Ss::BatchBuilder as BatchBuilder>::Spec as Spec>::Da,
            <<Ss::BatchBuilder as BatchBuilder>::Spec as Spec>::Address,
            <Ss::BatchBuilder as BatchBuilder>::Config,
        >,
        shutdown_receiver: tokio::sync::watch::Receiver<()>,
    ) -> anyhow::Result<(Self, Vec<tokio::task::JoinHandle<()>>)> {
        let (events_sender, _) = broadcast::channel(Self::EVENTS_CHANNEL_SIZE);

        let latest_state_update = state_update_receiver.borrow().clone();
        let latest_processed_slot_number = latest_state_update.slot_number;

        let tx_status_manager = TxStatusManager::default();

        let (batch_builder, maybe_bb_join_handle) = Ss::BatchBuilder::create(
            latest_state_update,
            tx_status_manager.clone(),
            da_sync_state.clone(),
            storage_path,
            config,
        )
        .await?;

        let api_state = batch_builder.api_state();

        let sequencer = Self {
            inner: Arc::new(Inner {
                batch_builder: Mutex::new(batch_builder),
                api_state,
                events_sender,
                da_service,
                tx_status_manager,
            }),
        };

        let background_handle = tokio::spawn({
            let s = sequencer.clone();
            let automatic_batch_production = config.automatic_batch_production;

            async move {
                if let Err(error) = s
                    .loop_background_task(
                        state_update_receiver,
                        latest_processed_slot_number,
                        ledger_db,
                        shutdown_receiver,
                        automatic_batch_production,
                    )
                    .await
                {
                    error!(%error, "Sequencer background task failed");
                }
            }
        });

        let mut handles = vec![background_handle];
        if let Some(bb_handle) = maybe_bb_join_handle {
            handles.push(bb_handle);
        }

        Ok((sequencer, handles))
    }

    /// Locks the batch builder and returns a reference to it.
    pub async fn batch_builder(&self) -> MutexGuard<Ss::BatchBuilder> {
        self.inner.batch_builder.lock().await
    }

    /// Subscribes to events emitted by the sequencer.
    pub async fn subscribe_events(&self) -> broadcast::Receiver<SequencerEvent<Ss::BatchBuilder>> {
        self.events_sender.subscribe()
    }

    /// Checks whether the sequencer is ready to accept transactions.
    pub async fn is_ready(&self) -> Result<(), SequencerNotReadyDetails> {
        self.batch_builder().await.is_ready()
    }

    pub(crate) fn tx_status_manager(&self) -> &TxStatusManager<<Ss::Da as DaService>::Spec> {
        &self.tx_status_manager
    }

    /// Get the latest API state from the batch builder
    pub fn api_state(&self) -> ApiState<<Ss::BatchBuilder as BatchBuilder>::Spec> {
        self.api_state.clone()
    }

    /// Encodes the transaction into the format accepted by [`BatchBuilder::accept_tx`].
    ///
    /// TODO(@neysofu): this method should be replaced an API endpoint -aware
    /// approach, so that multiple transaction formats can be supported.
    pub fn encode_tx(&self, raw: RawTx) -> FullyBakedTx {
        Ss::BatchBuilder::encode_tx(raw)
    }

    /// See [`BatchBuilder::accept_tx`].
    #[tracing::instrument(skip_all)]
    pub async fn accept_tx(
        &self,
        tx: FullyBakedTx,
    ) -> Result<AcceptedTx<<Ss::BatchBuilder as BatchBuilder>::Confirmation>, ErrorObject> {
        let mut batch_builder = self.batch_builder().await;

        self.accept_tx_and_notify(&mut batch_builder, tx).await
    }

    /// Calls [`BatchBuilder::accept_tx`] for each transaction, and finally
    /// [`BatchBuilder::assemble_batch`].
    #[tracing::instrument(skip_all)]
    pub async fn submit_batch(
        &self,
        txs: Vec<FullyBakedTx>,
    ) -> anyhow::Result<SubmitBatchReceipt<<Ss::Da as DaService>::Spec>> {
        tracing::info!(
            txs_count = txs.len(),
            "Submit batch request has been received!"
        );

        let mut batch_builder = self.batch_builder().await;

        for tx in txs {
            // TODO(@neysofu): information about transaction failures is lost...
            // it'd be nice to add it to the response, but at the same time
            // we're thinking of deprecating or removing. `POST
            // /sequencer/batches`. Gotta figure out what to do here.
            self.accept_tx_and_notify(&mut batch_builder, tx.clone())
                .await
                .ok();
        }

        batch_builder.assemble_batch().await?;
        self.send_all_unsent_batches(&mut batch_builder).await
    }

    /// Queries the latest known status of the given transaction. Best-effort,
    /// can't promise to always know the status.
    pub async fn tx_status(
        &self,
        tx_hash: &TxHash,
    ) -> anyhow::Result<TxStatus<<<Ss::Da as DaService>::Spec as DaSpec>::TransactionId>> {
        // Hit the cache...
        if let Some(status) = self.tx_status_manager.get_cached(tx_hash) {
            Ok(status)
        } else {
            // ...and then the batch builder's database.
            self.batch_builder().await.tx_status(tx_hash).await
        }
    }

    async fn send_all_unsent_batches(
        &self,
        batch_builder: &mut Ss::BatchBuilder,
    ) -> anyhow::Result<SubmitBatchReceipt<<Ss::Da as DaService>::Spec>> {
        let mut batches = batch_builder.peek_batches().await?;

        let Some(last_batch) = batches.pop() else {
            panic!("Not a single batch was available for sending, but this is unexpected; this is a bug, please report it");
        };

        for batch in batches {
            let receipt_fut = self.inner.send_batch(batch_builder, batch).await?;
            let seq = self.clone();

            let handle = tokio::spawn(async move {
                if let Err(error) = seq.react_to_batch_receipt(receipt_fut).await {
                    error!(%error, "Failed to react to batch receipt; this is likely a bug, please report it");
                }
            });

            if !Ss::BatchBuilder::PARALLEL_DA_SUBMISSION {
                handle
                    .await
                    .expect("Failed to .await a task; this is a bug, please report it");
            }
        }

        self.react_to_batch_receipt(self.inner.send_batch(batch_builder, last_batch).await?)
            .await
    }

    async fn react_to_batch_receipt(
        &self,
        receipt_fut: WithCachedTxHashes<BlobReceiptFut<Ss>>,
    ) -> anyhow::Result<SubmitBatchReceipt<<Ss::Da as DaService>::Spec>> {
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
            self.tx_status_manager.notify(
                *tx_hash,
                TxStatus::Published {
                    da_tx_id: receipt.da_transaction_id.clone(),
                },
            );
        }

        Ok(SubmitBatchReceipt {
            tx_hashes: receipt_fut.tx_hashes,
            submit_blob_receipt: receipt,
        })
    }
}

/// Private background loop -related code.
impl<Ss: SequencerSpec> Sequencer<Ss> {
    #[tracing::instrument(skip_all)]
    async fn loop_background_task(
        &self,
        mut state_update_receiver: StateUpdateReceiver<
            <<Ss::BatchBuilder as BatchBuilder>::Spec as Spec>::Storage,
        >,
        mut latest_processed_slot_number: SlotNumber,
        ledger_db: LedgerDb,
        shutdown_receiver: tokio::sync::watch::Receiver<()>,
        automatic_batch_production: bool,
    ) -> anyhow::Result<()> {
        loop {
            let fut = future_or_shutdown(state_update_receiver.changed(), &shutdown_receiver);
            let changed = match fut.await {
                FutureOrShutdownOutput::Output(c) => c,
                FutureOrShutdownOutput::Shutdown => {
                    info!("Shutting down sequencer background task...");
                    break;
                }
            };

            if let Err(error) = changed {
                tracing::error!(%error, "Channel notification failed");
                continue;
            }

            // Remember: we are dealing with a `watch::Receiver`, so > 1 num. of
            // values MAY have been produced since the last time we took this
            // code path. We MUST assume that some updates MAY be skipped (not
            // *lost*, but *skipped* as in "superseded by a newer value").

            let info = (*state_update_receiver.borrow()).clone();
            self.handle_state_update_info(
                info,
                &mut latest_processed_slot_number,
                &ledger_db,
                automatic_batch_production,
            )
            .await?;
        }

        debug!("The background loop of the sequencer is shutting down");
        Ok(())
    }

    async fn handle_state_update_info(
        &self,
        state_update_info: StateUpdateInfo<
            <<Ss::BatchBuilder as BatchBuilder>::Spec as Spec>::Storage,
        >,
        latest_processed_slot_number: &mut SlotNumber,
        ledger_db: &LedgerDb,
        automatic_batch_production: bool,
    ) -> anyhow::Result<()> {
        // Update storage. It is scoped, so batch builder lock is released early.
        let storage_slot_number = {
            let slot_number = state_update_info.slot_number;
            let mut bb = self.batch_builder().await;
            bb.update_state(state_update_info.clone()).await;
            slot_number
        };

        self.notify_processed_slots(
            ledger_db,
            latest_processed_slot_number.range_inclusive(storage_slot_number),
        )
        .await?;

        *latest_processed_slot_number = state_update_info.slot_number;

        // Now that we retrieved the latest state, we can produce and send a new batch.
        if automatic_batch_production {
            tracing::trace!("Producing a batch automatically");
            // No additional transactions, the batches will
            // just contain whatever transactions have been accepted already
            // (possibly none).
            let txs = vec![];
            self.submit_batch(txs).await?;
        }

        *latest_processed_slot_number = state_update_info.slot_number;

        Ok(())
    }

    async fn notify_processed_slots(
        &self,
        ledger_db: &LedgerDb,
        slot_number_range: impl Iterator<Item = SlotNumber>,
    ) -> anyhow::Result<()> {
        for slot_number in slot_number_range {
            let slot = ledger_db
                .get_slot_by_number::<Ss::BatchReceipt, Ss::TxReceipt, Ss::Event>(
                    slot_number,
                    QueryMode::Full,
                )
                .await?
                .unwrap();
            self.notify_processed_slot(slot).await?;
        }

        Ok(())
    }

    async fn notify_processed_slot(
        &self,
        slot: SlotResponse<Ss::BatchReceipt, Ss::TxReceipt, Ss::Event>,
    ) -> anyhow::Result<()> {
        for batch in slot.batches.unwrap_or_default().iter() {
            let ItemOrHash::Full(batch) = batch else {
                continue;
            };
            for tx in batch.txs.as_deref().unwrap_or_default().iter() {
                let ItemOrHash::Full(tx) = tx else {
                    continue;
                };

                self.tx_status_manager
                    .notify(TxHash::new(tx.hash), TxStatus::Processed);
            }
        }

        Ok(())
    }
}

impl<Ss: SequencerSpec> Inner<Ss> {
    async fn accept_tx_and_notify(
        &self,
        batch_builder: &mut Ss::BatchBuilder,
        tx: FullyBakedTx,
    ) -> Result<AcceptedTx<<Ss::BatchBuilder as BatchBuilder>::Confirmation>, ErrorObject> {
        debug!(tx = hex::encode(&tx.data), "Accepting transaction");

        let accepted = batch_builder.accept_tx(tx).await?;
        self.notify_accepted_tx(&accepted);

        Ok(accepted)
    }

    fn notify_accepted_tx(
        &self,
        tx: &AcceptedTx<<Ss::BatchBuilder as BatchBuilder>::Confirmation>,
    ) {
        // It makes sense to me (@neysofu) that tx status notifications are sent
        // before events, but I can see arguments for both.
        self.tx_status_manager
            .notify(tx.tx_hash, TxStatus::Submitted);

        for event in tx.confirmation.events() {
            self.events_sender
                .send(SequencerEvent {
                    tx_hash: tx.tx_hash,
                    event,
                })
                .ok();
        }
    }

    async fn send_batch(
        &self,
        batch_builder: &mut Ss::BatchBuilder,
        batch: WithCachedTxHashes<<Ss::BatchBuilder as BatchBuilder>::Batch>,
    ) -> anyhow::Result<WithCachedTxHashes<BlobReceiptFut<Ss>>> {
        let WithCachedTxHashes {
            inner: next_batch,
            tx_hashes,
        } = batch;

        let serialized_batch = borsh::to_vec(&next_batch)
            .expect("Failed to serialize batch inside sequencer; this is a bug, please report it");

        let fee = match self.da_service.estimate_fee(serialized_batch.len()).await {
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

        let receipt_fut = self
            .da_service
            .send_transaction(&serialized_batch, fee)
            .await;

        // If we crash here, the batch will still be sitting inside the batch
        // builder's database and it will be re-submitted once again. Not ideal,
        // but certainly better than losing it forever. This is the correct
        // behavior.

        batch_builder.pop_batch().await?;

        Ok(WithCachedTxHashes {
            inner: receipt_fut,
            tx_hashes,
        })
    }
}

type BlobReceiptFut<Ss> = oneshot::Receiver<
    Result<
        SubmitBlobReceipt<
            <<<Ss as SequencerSpec>::Da as DaService>::Spec as DaSpec>::TransactionId,
        >,
        <<Ss as SequencerSpec>::Da as DaService>::Error,
    >,
>;

#[derive(Debug, serde::Serialize)]
pub struct SequencerNotReadyDetails {
    pub target_da_height: u64,
    pub synced_da_height: u64,
}

#[derive(derivative::Derivative, serde::Serialize, serde::Deserialize)]
#[derivative(Clone(bound = ""))]
#[serde(bound = "")]
pub struct SequencerEvent<Bb: BatchBuilder> {
    tx_hash: TxHash,
    #[serde(flatten)]
    event: RuntimeEventResponse<
        <<Bb as BatchBuilder>::Confirmation as SequencerConfirmation>::EventInner,
    >,
}

/// An object-safe interface to the preferred sequencer, which can be used to
/// get a sequence number assigned to preferred proof blobs.
#[async_trait]
pub trait SequenceNumberProvider: Send + Sync + 'static {
    /// Generates the next sequence number to use for a new preferred proof blob.
    ///
    /// Subsequent calls to this method MUST return different (greater) values.
    async fn generate_sequence_number(&self, preferred_blob: &[u8]) -> anyhow::Result<u64>;
}
