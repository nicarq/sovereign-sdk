use std::sync::Arc;

use axum::async_trait;
use sov_db::ledger_db::LedgerDb;
use sov_db::sequencer_db::{SequenceNumber, SequencerDb};
use sov_modules_api::rest::{ApiState, StateUpdateReceiver};
use sov_modules_api::{
    DaSyncState, FullyBakedTx, RawTx, RuntimeEventResponse, Spec, StateUpdateInfo,
};
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::node::ledger_api::{
    ItemOrHash, LedgerStateProvider, QueryMode, SlotResponse,
};
use sov_rollup_interface::node::{future_or_shutdown, FutureOrShutdownOutput};
use sov_rollup_interface::TxHash;
use tokio::sync::{broadcast, Mutex, MutexGuard};
use tracing::{debug, error, info};

use super::tx_status::{TxStatus, TxStatusManager};
use super::SubmitBatchReceipt;
use crate::batch_builders::preferred::PreferredBatchBuilder;
use crate::batch_builders::{
    AcceptTxError, AcceptedTx, BatchBuilder, FreshlyBuiltBatch, RtAwareBatchBuilderSpec,
    SequencerConfirmation,
};
use crate::{SeqDbTx, SeqDbTxExtend, SequencerConfig, SequencerSpec};

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
    sequencer_db: SequencerDb,
    events_sender: broadcast::Sender<SequencerEvent<Ss::BatchBuilder>>,
    da_service: Ss::Da,
    tx_status_manager: TxStatusManager<<Ss::Da as DaService>::Spec>,
}

impl<Ss: SequencerSpec> Inner<Ss> {
    async fn build_and_send_batch(
        &self,
        batch_builder: &mut MutexGuard<'_, Ss::BatchBuilder>,
    ) -> anyhow::Result<SubmitBatchReceipt<<Ss::Da as DaService>::Spec>> {
        let sequence_number = self
            .sequencer_db
            .get_and_increase_next_sequence_number()
            .await?;

        // FIXME: if the node crashes here, the sequence number is lost forever
        // and the node can't recover.

        let FreshlyBuiltBatch {
            inner: next_batch,
            hashes: tx_hashes,
        } = batch_builder.build_next_batch(sequence_number).await?;
        let serialized_batch = borsh::to_vec(&next_batch)
            .expect("Failed to serialize batch inside sequencer; this is a bug, please report it");

        let fee = match self.da_service.estimate_fee(serialized_batch.len()).await {
            Ok(fee) => fee,
            Err(e) => anyhow::bail!(
                "failed to submit batch: could not determine appropriate fee rate: {}",
                e
            ),
        };

        let submit_blob_receipt = match self
            .da_service
            .send_transaction(&serialized_batch, fee)
            .await
            .await
            .expect("The transaction sender should not fail")
        {
            Ok(id) => id,
            Err(e) => anyhow::bail!("failed to submit batch: {}", e),
        };

        batch_builder.clear_batch().await?;
        self.sequencer_db.remove(&tx_hashes)?;

        for tx_hash in &tx_hashes {
            self.tx_status_manager.notify(
                *tx_hash,
                TxStatus::Published {
                    da_tx_id: submit_blob_receipt.da_transaction_id.clone(),
                },
            );
        }

        let receipt = SubmitBatchReceipt {
            tx_hashes,
            submit_blob_receipt,
        };
        tracing::debug!(?receipt, "Batch has been build and sent");

        Ok(receipt)
    }

    async fn produce_batch(&self) -> anyhow::Result<()> {
        let mut batch_builder = self.batch_builder.lock().await;
        self.build_and_send_batch(&mut batch_builder).await?;

        Ok(())
    }
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
        sequencer_db: SequencerDb,
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
        let latest_processed_rollup_height = latest_state_update.rollup_height;

        let (batch_builder, maybe_bb_join_handle) = Ss::BatchBuilder::create(
            latest_state_update,
            da_sync_state.clone(),
            sequencer_db.read_all()?,
            config,
        )
        .await?;

        let tx_status_manager = batch_builder.tx_status_manager();
        let api_state = batch_builder.api_state();

        let sequencer = Self {
            inner: Arc::new(Inner {
                batch_builder: Mutex::new(batch_builder),
                api_state,
                events_sender,
                sequencer_db,
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
                        latest_processed_rollup_height,
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

    /// Returns a reference to the underlying [`SequencerDb`].
    pub fn db(&self) -> &SequencerDb {
        &self.inner.sequencer_db
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
        self.batch_builder.lock().await.is_ready()
    }

    pub(crate) fn tx_status_manager(&self) -> &TxStatusManager<<Ss::Da as DaService>::Spec> {
        &self.tx_status_manager
    }

    /// Get the latest API state from the batch builder
    pub fn api_state(&self) -> ApiState<<Ss::BatchBuilder as BatchBuilder>::Spec> {
        self.api_state.clone()
    }

    /// Calls [`BatchBuilder::accept_tx`] for each transaction, and finally
    /// [`BatchBuilder::build_next_batch`].
    pub async fn submit_batch(
        &self,
        txs: Vec<FullyBakedTx>,
    ) -> anyhow::Result<SubmitBatchReceipt<<Ss::Da as DaService>::Spec>> {
        tracing::info!("Submit batch request has been received!");
        let mut batch_builder = self.batch_builder().await;

        let mut accept_tx_results = vec![];
        for tx in txs {
            let mut result = batch_builder.accept_tx(tx.clone()).await;

            match &result {
                Ok(accepted) => {
                    let stored_tx = SeqDbTx::new(accepted.tx_hash, tx);

                    if let Err(e) = self.sequencer_db.insert(&stored_tx).await {
                        error!(%e, "Database error. Failed to add transaction to batch");
                        result = Err(AcceptTxError {
                            http_status: 500,
                            title: "Database Error".to_string(),
                            details: String::new(),
                        });
                    } else {
                        self.notify_accepted_tx(accepted);
                    }
                }
                Err(_) => {}
            }

            accept_tx_results.push(result);
        }

        self.inner.build_and_send_batch(&mut batch_builder).await
    }

    /// Encodes the transaction into the format accepted by [`BatchBuilder::accept_tx`].
    ///
    /// TODO(@neysofu): this method should be replaced an API endpoint -aware
    /// approach, so that multiple transaction formats can be supported.
    pub fn encode_tx(&self, raw: RawTx) -> FullyBakedTx {
        Ss::BatchBuilder::encode_tx(raw)
    }

    /// See [`BatchBuilder::accept_tx`].
    pub async fn accept_tx(
        &self,
        baked_tx: FullyBakedTx,
    ) -> Result<AcceptedTx<<Ss::BatchBuilder as BatchBuilder>::Confirmation>, AcceptTxError> {
        tracing::info!(tx = hex::encode(&baked_tx.data), "Accepting transaction");
        let mut batch_builder = self.batch_builder().await;

        let accepted = batch_builder.accept_tx(baked_tx.clone()).await?;
        let stored_tx = SeqDbTx::new(accepted.tx_hash, baked_tx);

        self.sequencer_db.insert(&stored_tx).await.map_err(|e| {
            error!(%e, "Database error. Failed to accept transaction");
            AcceptTxError {
                http_status: 500,
                title: "Database Error".to_string(),
                details: String::new(),
            }
        })?;
        self.notify_accepted_tx(&accepted);

        Ok(accepted)
    }

    /// Queries the latest known status of the given transaction. Best-effort,
    /// can't promise to always know the status.
    pub async fn tx_status(
        &self,
        tx_hash: &TxHash,
    ) -> anyhow::Result<Option<TxStatus<<<Ss::Da as DaService>::Spec as DaSpec>::TransactionId>>>
    {
        // TODO: This report is not completely accurate. The mempool is allowed to drop transactions
        // but currently has no mechanism to remove them from the sequencer_db, so there can be a window
        // between the time that a tx is evicted from the notificaiton cache and the time its entry is
        // TTL'd where it will report `Submitted` instead of `Dropped`
        if let Some(status) = self.tx_status_manager.get_cached(tx_hash) {
            return Ok(Some(status));
        } else if self.sequencer_db.get(tx_hash).await?.is_some() {
            return Ok(Some(TxStatus::Submitted));
        }
        Ok(None)
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
        mut latest_processed_rollup_height: u64,
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

            if changed.is_err() {
                tracing::debug!("Error in state update");
                continue;
            }

            // Remember: we are dealing with a `watch::Receiver`, so > 1 num. of
            // values MAY have been produced since the last time we took this
            // code path. We MUST assume that some updates MAY be skipped (not
            // *lost*, but *skipped* as in "superseded by a newer value").

            let info = (*state_update_receiver.borrow()).clone();
            self.handle_state_update_info(
                info,
                &mut latest_processed_rollup_height,
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
        latest_processed_rollup_height: &mut u64,
        ledger_db: &LedgerDb,
        automatic_batch_production: bool,
    ) -> anyhow::Result<()> {
        // Update storage. It is scoped, so batch builder lock is released early.
        let storage_rollup_height = {
            let rollup_height = state_update_info.rollup_height;
            let mut bb = self.batch_builder().await;
            bb.update_state(state_update_info.clone()).await;
            rollup_height
        };

        self.notify_processed_slots(
            ledger_db,
            *latest_processed_rollup_height..=storage_rollup_height,
        )
        .await?;

        // Now that we retrieved the latest state, we can produce and send a new batch.
        if automatic_batch_production {
            tracing::debug!("Producing a batch");
            self.produce_batch().await?;
        }

        *latest_processed_rollup_height = state_update_info.rollup_height;

        Ok(())
    }

    async fn notify_processed_slots(
        &self,
        ledger_db: &LedgerDb,
        rollup_height_range: impl Iterator<Item = u64>,
    ) -> anyhow::Result<()> {
        for rollup_height in rollup_height_range {
            let slot = ledger_db
                .get_slot_by_rollup_height::<Ss::BatchReceipt, Ss::TxReceipt, Ss::Event>(
                    rollup_height,
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

    fn notify_accepted_tx(
        &self,
        tx: &AcceptedTx<<Ss::BatchBuilder as BatchBuilder>::Confirmation>,
    ) {
        for event in tx.confirmation.events() {
            self.events_sender
                .send(SequencerEvent {
                    tx_hash: tx.tx_hash,
                    event,
                })
                .ok();
        }

        self.tx_status_manager
            .notify(tx.tx_hash, TxStatus::Submitted);
    }
}

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
    /// Returns the next sequence number.
    ///
    /// Subsequent calls to this method MUST return different (greater) values.
    async fn next_sequence_number(&self, preferred_blob: &[u8]) -> anyhow::Result<SequenceNumber>;
}

#[async_trait]
impl<Z, Ss> SequenceNumberProvider for Sequencer<Ss>
where
    Z: RtAwareBatchBuilderSpec,
    Ss: SequencerSpec<BatchBuilder = PreferredBatchBuilder<Z>>,
    //                               ^^^^^^^^^^^^^^^^^^^^^^^^
    // One should not be able to use a non-preferred sequencer to produce
    // sequence numbers.
{
    async fn next_sequence_number(&self, _preferred_blob: &[u8]) -> anyhow::Result<SequenceNumber> {
        self.sequencer_db
            .get_and_increase_next_sequence_number()
            .await
    }
}
