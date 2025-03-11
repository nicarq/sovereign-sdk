//! See [`PreferredSequencer`].

mod async_batch;
mod block_executor;
mod db;

use std::marker::PhantomData;
use std::num::NonZero;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use async_trait::async_trait;
use db::postgres::PostgresBackend;
use db::rocksdb::RocksDbBackend;
use db::{PreferredSequencerDb, PreferredSequencerDbBackend, PreferredSequencerReadBlob};
use schemars::JsonSchema;
use serde_with::serde_as;
use sov_blob_storage::PreferredBatchData;
use sov_db::ledger_db::LedgerDb;
use sov_modules_api::capabilities::BlobSelector;
use sov_modules_api::rest::utils::ErrorObject;
use sov_modules_api::rest::{ApiState, StateUpdateReceiver};
use sov_modules_api::{
    ApiTxEffect, ChangeSet, FullyBakedTx, RejectReason, Runtime, RuntimeEventProcessor,
    RuntimeEventResponse, Spec, StateCheckpoint, StateUpdateInfo, SyncStatus, TxChangeSet,
    VersionReader, VisibleSlotNumber,
};
use sov_modules_stf_blueprint::{TransactionReceipt, TxReceiptContents};
use sov_rest_utils::errors::database_error_500;
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::node::DaSyncState;
use sov_rollup_interface::TxHash;
use sov_state::{NativeStorage, Storage};
use tokio::sync::mpsc::{self, Sender};
use tokio::sync::{broadcast, watch, Mutex, MutexGuard};
use tokio::task::JoinHandle;
use tracing::{debug, error, trace, Instrument};

use crate::blob_sender::BlobSender;
use crate::common::{
    loop_call_update_state, loop_send_tx_notifications, AcceptedTx, Sequencer, WithCachedTxHashes,
};
use crate::preferred::block_executor::{RollupBlockExecutor, RollupBlockExecutorError};
use crate::{
    SequenceNumberProvider, SequencerConfig, SequencerEvent, SequencerNotReadyDetails,
    SubmitBatchReceipt, TxStatus, TxStatusManager,
};

type VisibleSlotNumberIncrease = NonZero<u8>;

/// A inner batch builder struct containing state that requires synchronized access.
struct Inner<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    db: PreferredSequencerDb<S, Rt>,
    latest_info: StateUpdateInfo<S::Storage>,
    checkpoint_sender: watch::Sender<StateCheckpoint<S>>,
    next_event_number: u64,
    blob_sender: BlobSender<Da, PreferredBatchData>,
    config: SequencerConfig<S::Da, S::Address, PreferredSequencerConfig>,
    block_executor: RollupBlockExecutor<S, Rt>,
}

impl<S, Rt, Da> Inner<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    /// Syncs [`ApiState`]s with the latest [`StateCheckpoint`].
    #[tracing::instrument(skip_all, level = "trace")]
    async fn update_api_state(&self) {
        self.checkpoint_sender.send(
            self.block_executor.state().checkpoint_ref().clone_with_empty_witness()
        ).expect("sending the checkpoint should never fail because one receiver is always present; this is a bug, please report it");
    }

    fn node_root_hash(&self) -> anyhow::Result<<S::Storage as Storage>::Root> {
        self.latest_info
            .storage
            .get_root_hash(self.latest_info.slot_number)
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn try_to_create_and_start_batch_if_none_in_progress(
        &mut self,
        leave_space_for_next_batch: bool,
    ) -> Result<Option<()>, ErrorObject> {
        let checkpoint = match &self.block_executor.state() {
            InternalState::Idle { checkpoint, .. } => checkpoint,
            InternalState::InProgressBatch { .. } => return Ok(Some(())),
            InternalState::Placeholder => panic!("The sequencer contains an invalid internal state. This is a bug, please report it."),
        };

        let Ok(visible_increase) = next_visible_slot_number_increase(
            checkpoint,
            &self.latest_info,
            leave_space_for_next_batch,
        ) else {
            return Ok(None);
        };

        debug!(visible_increase, "No in-progress batch, starting a new one");

        let node_state_root = self.node_root_hash().map_err(database_error_500)?;

        // If the database operation fails here it's okay because we still
        // haven't touched the background task nor modified `self`, so
        // everything will be left in a valid state.
        self.db
            .start_batch(
                VisibleSlotNumber::new_dangerous(
                    self.latest_info.latest_finalized_slot_number.get(),
                ),
                visible_increase,
            )
            .await
            .map_err(database_error_500)?;

        self.block_executor
            .start_rollup_block(
                visible_increase,
                None,
                &node_state_root,
                self.config.sequencer_kind_config.minimum_profit_per_tx,
            )
            .await;

        Ok(Some(()))
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn produce_batch_if_possible(
        &mut self,
    ) -> anyhow::Result<Option<WithCachedTxHashes<PreferredBatchData>>> {
        let checkpoint = self.block_executor.state().checkpoint_ref();

        // Check if we have enough slots to create a new batch immediately after
        // this one. If we don't, let's not assemble a batch.
        //
        // TODO(@neysofu): this check is currently necessary but likely can be folded into
        // `try_to_create_and_start_batch_if_none_in_progress`... somehow. As of
        // right now, it's a hair too bug-prone.
        if next_visible_slot_number_increase(checkpoint, &self.latest_info, true).is_err() {
            return Ok(None);
        }

        let new_batch_res = self.try_to_create_and_start_batch_if_none_in_progress(true)
            .await
            .map_err(|_| anyhow::anyhow!("Unable to start a new batch; this is likely a database issue or a bug, please report it"));

        if new_batch_res?.is_none() {
            return Ok(None);
        }

        let batch = self.db.terminate_batch().await?;
        self.block_executor.end_rollup_block_if_in_progress().await;

        self.update_api_state().await;
        Ok(Some(batch.into()))
    }
}

#[derive(derive_more::Debug)]
#[debug(bounds())]
enum InternalState<S: Spec> {
    /// Invalid state, used when we need to temporarily own the
    /// [`StateCheckpoint`].
    Placeholder,
    /// The [`Sequencer`] is currently idle and is not processing
    /// transactions for the next rollup block yet.
    Idle {
        checkpoint: StateCheckpoint<S>,
        /// When set to [`None`], the next rollup block is built on top of node
        /// state instead of sequencer state.
        ///
        /// See [`PreferredSequencer::latest_info`].
        prev_state_root_opt: Option<<S::Storage as Storage>::Root>,
    },
    /// The [`Sequencer`] is currently accepting transactions from the
    /// preferred batch of a rollup block. Note that every rollup block
    /// (under normal operations, not e.g. in recovery mode) has exactly one
    /// preferred batch.
    InProgressBatch {
        checkpoint: StateCheckpoint<S>,
        task_state: BackgroundTaskState<S>,
    },
}

impl<S: Spec> InternalState<S> {
    fn node(info: &StateUpdateInfo<S::Storage>, runtime: &mut impl Runtime<S>) -> Self {
        let checkpoint = StateCheckpoint::new(info.storage.clone(), &runtime.kernel());

        InternalState::Idle {
            checkpoint,
            prev_state_root_opt: None,
        }
    }

    pub fn checkpoint_ref(&self) -> &StateCheckpoint<S> {
        match self {
            InternalState::Idle { checkpoint, .. }
            | InternalState::InProgressBatch { checkpoint, .. } => checkpoint,
            InternalState::Placeholder => panic!("The sequencer contains an invalid internal state. This is a bug, please report it."),
        }
    }
}

/// A [`Sequencer`] with instant transaction confirmation.
pub struct PreferredSequencer<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    inner: Mutex<Inner<S, Rt, Da>>,
    tx_status_manager: TxStatusManager<S::Da>,
    events_sender: broadcast::Sender<SequencerEvent<Rt>>,
    api_state: ApiState<S>,
    da_sync_state: Arc<DaSyncState>,
    _runtime: PhantomData<Rt>,
    config: SequencerConfig<S::Da, S::Address, PreferredSequencerConfig>,
    has_been_ready: AtomicBool,
    shutdown_notifier: Sender<()>,
}

impl<S, Rt, Da> PreferredSequencer<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    #[tracing::instrument(skip_all, level = "debug")]
    async fn lock_inner(&self) -> MutexGuard<Inner<S, Rt, Da>> {
        self.inner.lock().await
    }
}

#[async_trait]
impl<S, Rt, Da> Sequencer for PreferredSequencer<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    type Confirmation = Confirmation<S, Rt>;
    type Config = PreferredSequencerConfig;
    type Spec = S;
    type Rt = Rt;
    type Da = Da;

    /// At the time of writing, the [`PreferredSequencer`] doesn't use
    /// the [`TxStatusManager`].
    ///
    /// The [`Sequencer`] itself already updates the
    /// [`TxStatusManager`] after all operations, so we'd only need it if we
    /// ever "drop" previously-accepted transactions. The whole point of the
    /// [`PreferredSequencer`] is that we *don't* do that.
    async fn create(
        da: Da,
        state_update_receiver: StateUpdateReceiver<S::Storage>,
        da_sync_state: Arc<DaSyncState>,
        storage_path: &Path,
        config: &SequencerConfig<S::Da, S::Address, Self::Config>,
        ledger_db: LedgerDb,
        shutdown_receiver: watch::Receiver<()>,
    ) -> anyhow::Result<(Arc<Self>, Vec<JoinHandle<()>>)> {
        let latest_state_update = state_update_receiver.borrow().clone();
        debug!(
            ?latest_state_update,
            "Instantiating the preferred batch builder"
        );

        let mut runtime: Rt = Default::default();
        let tx_status_manager = TxStatusManager::default();

        assert!(
            accepts_preferred_batches(runtime.blob_selector()),
            "Attempting to use preferred sequencer with an incompatible rollup. Set your sequencer config to `standard` in your rollup's config.toml file or change your kernel to be compatible with soft confirmations."
        );

        let (checkpoint_sender, checkpoint_receiver) = watch::channel(StateCheckpoint::new(
            latest_state_update.storage.clone(),
            &runtime.kernel(),
        ));
        let api_state = ApiState::build(
            Arc::new(()),
            checkpoint_receiver,
            runtime.kernel_with_slot_mapping(),
            None,
        );

        let (shutdown_notifier, mut shutdown_rx) = mpsc::channel(1);
        let mut handles = vec![tokio::task::spawn(async move {
            // This task blocks until we receive a notification that all
            // background tasks have been shut down.
            let _ = shutdown_rx.recv().await;
        })];

        let (events_sender, _) =
            broadcast::channel(config.sequencer_kind_config.events_channel_size);

        let db_backend: Box<dyn PreferredSequencerDbBackend> =
            if let Some(postgres_connection_string) =
                &config.sequencer_kind_config.postgres_connection_string
            {
                Box::new(PostgresBackend::connect(postgres_connection_string).await?)
            } else {
                Box::new(RocksDbBackend::new(storage_path).await?)
            };

        let inner = Inner {
            db: PreferredSequencerDb::<S, Rt>::new(db_backend).await?,
            latest_info: latest_state_update.clone(),
            checkpoint_sender,
            next_event_number: latest_state_update.next_event_number,
            config: config.clone(),
            block_executor: RollupBlockExecutor::new(
                InternalState::Placeholder,
                config.clone(),
                shutdown_notifier.clone(),
            ),
            blob_sender: BlobSender::new(
                da,
                storage_path,
                tx_status_manager.clone(),
                true,
                shutdown_receiver.clone(),
            )
            .await?,
        };

        let seq = Arc::new(PreferredSequencer {
            inner: inner.into(),
            tx_status_manager,
            events_sender,
            da_sync_state,
            api_state,
            _runtime: PhantomData,
            shutdown_notifier,
            config: config.clone(),
            has_been_ready: AtomicBool::new(false),
        });

        seq.update_state(latest_state_update.clone())
            .await
            .expect("Failed to initialize sequencer state from node");

        handles.push(tokio::spawn({
            loop_call_update_state(
                seq.clone(),
                state_update_receiver.clone(),
                shutdown_receiver.clone(),
            )
        }));
        handles.push(tokio::spawn({
            let ledger_db = ledger_db.clone();
            let seq = seq.clone();
            async move {
                loop_send_tx_notifications::<S, Rt>(
                    state_update_receiver,
                    shutdown_receiver,
                    &ledger_db,
                    seq.tx_status_manager(),
                )
                .await;
            }
        }));

        Ok((seq, handles))
    }

    async fn is_ready(&self) -> Result<(), SequencerNotReadyDetails> {
        // On startup, we need to wait for enough finalized data to be available. In this case,
        // we have to do a more expensive check where we check if we have a finalized slot number
        // available. Since this requires locking, we skip this check on the
        // fast path after genesis.
        if !self
            .has_been_ready
            .load(std::sync::atomic::Ordering::Acquire)
        {
            let inner = self.lock_inner().await;
            match &inner.block_executor.state() {
                InternalState::Idle { checkpoint, .. } => {
                    next_visible_slot_number_increase(checkpoint, &inner.latest_info, false)?;
                }
                InternalState::Placeholder => {
                    panic!("Sequencer is in placeholder state during readiness check. This is a bug, please report it.");
                }
                // If the sequencer has started a batch already, then it's ready.
                InternalState::InProgressBatch { .. } => {}
            };

            self.has_been_ready
                .store(true, std::sync::atomic::Ordering::Release);
        }
        let status = self.da_sync_state.status();

        match status {
            SyncStatus::Synced { .. } => Ok(()),
            SyncStatus::Syncing {
                synced_da_height,
                target_da_height,
            } => {
                let distance = status.distance();
                if distance <= sov_blob_storage::config_deferred_slots_count() {
                    Ok(())
                } else {
                    Err(SequencerNotReadyDetails::Syncing {
                        target_da_height,
                        synced_da_height,
                    })
                }
            }
        }
    }

    fn api_state(&self) -> ApiState<Self::Spec> {
        self.api_state.clone()
    }

    #[tracing::instrument(skip_all, level = "debug")]
    async fn update_state(&self, info: StateUpdateInfo<S::Storage>) -> anyhow::Result<()> {
        let batches_to_process = {
            let mut inner = self.lock_inner().await;

            batches_to_process(&mut inner.db, &info).await?
        };

        if tracing::enabled!(tracing::Level::TRACE) {
            let batch_details_to_log = batches_to_process
                .iter()
                .map(|batch| {
                    (
                        batch.batch.inner.sequence_number,
                        batch.batch.inner.visible_slots_to_advance,
                        batch.batch.inner.data.len(),
                    )
                })
                .collect::<Vec<_>>();
            trace!(
                ?batch_details_to_log,
                "Prepared batches to apply to the state"
            );
        }

        let mut executor = RollupBlockExecutor::<_, Rt>::new(
            InternalState::node(&info, &mut Rt::default()),
            self.config.clone(),
            self.shutdown_notifier.clone(),
        );

        let node_state_root = tracing::trace_span!("root_hash")
            .in_scope(|| info.storage.get_root_hash(info.slot_number))?;
        let last_batch = batches_to_process.last();
        let last_replayed_batch_in_progress = last_batch.map(|batch| batch.is_in_progress);
        let latest_batch_txs_len = last_batch.map(|batch| batch.batch.tx_hashes.len());

        async {
            for batch in batches_to_process {
                executor.replay_batch(&batch, &node_state_root).await?;
            }
            Ok::<(), anyhow::Error>(())
        }
        .instrument(tracing::debug_span!("process_batches"))
        .await?;

        // We stop accepting new txns in accept_tx for a short time while we catch up
        let mut inner = self.lock_inner().await;
        let current_in_progress_batch = inner.db.in_progress_batch_opt().await?.cloned();

        // Currently it's not possible for `accept_tx` to end a batch, this will likely
        // change in the future when it can close batches due to gas, stake, batch sizes, etc.
        // When that happens we'll also need to handle the case where `accept_tx` closes the batch.
        match (last_replayed_batch_in_progress, current_in_progress_batch) {
            // We have an in-progress batch, see if there's any new additions
            // since we've replayed the batches on the nodes state
            (Some(true), Some(batch)) => {
                let prev_txs_len =
                    latest_batch_txs_len.expect("In progress check was Some but txs len was None");
                let new_txs = batch.txs[prev_txs_len..].to_vec();

                trace!(new_txs = new_txs.len(), "Applying any new transactions have been added to in-progress batch while updating node state");

                for tx in new_txs {
                    let _ = executor.apply_tx_to_in_progress_batch(&tx).await;
                }
            }
            // There wasn't an in-progress batch previously but there is one now
            // It was started by accept_tx, lets add it to our state
            (_, Some(in_progress_batch)) => {
                trace!("Replaying batch that was initialized while updating node state");
                let batch = PreferredBatchToRestore {
                    is_in_progress: true,
                    visible_slot_number_after_increase: in_progress_batch
                        .visible_slot_number_after_increase,
                    batch: in_progress_batch.into(),
                };
                let node_root = inner.node_root_hash()?;
                executor.replay_batch(&batch, &node_root).await?;
            }
            _ => trace!("No new transaction or batch state while updating node state"),
        }

        trace!("Node state update complete, swapping new state into sequencer");
        inner.latest_info = info;
        inner.block_executor.replace_state(executor.consume()).await;
        inner.update_api_state().await;
        trace!("Node state update completed successfully");

        if self.config.automatic_batch_production {
            if let Some(batch) = inner.produce_batch_if_possible().await? {
                self.has_been_ready
                    .store(true, std::sync::atomic::Ordering::Release);
                // TODO(@ross-weir) #2534 Shouldn't need to hold the lock for this
                inner.blob_sender.publish_batch_and_wait(batch).await?;
            }
        }

        Ok(())
    }

    fn tx_status_manager(&self) -> &TxStatusManager<<Self::Spec as Spec>::Da> {
        &self.tx_status_manager
    }

    async fn subscribe_events(&self) -> Option<broadcast::Receiver<SequencerEvent<Rt>>> {
        Some(self.events_sender.subscribe())
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn accept_tx(
        &self,
        baked_tx: FullyBakedTx,
    ) -> Result<AcceptedTx<Self::Confirmation>, ErrorObject> {
        let mut inner = self.lock_inner().await;
        if inner
            .try_to_create_and_start_batch_if_none_in_progress(false)
            .await?
            .is_none()
        {
            panic!("No batch in progress, and no batch can be started. This is either because of (1) a bug, or (2) misuse of the `POST /sequencer/batches` endpoint. Please use automatic batch production exclusively, and report this bug if necessary. {:?} {:?}", inner.block_executor.state(), inner.latest_info);
        }

        let receipt = inner
            .block_executor
            .apply_tx_to_in_progress_batch(&baked_tx)
            .await
            .map_err(RollupBlockExecutorError::into_http_error)?;
        inner
            .db
            .insert_tx(baked_tx.clone(), receipt.tx_hash)
            .await
            .map_err(database_error_500)?;

        let events_len = receipt.events.len() as u64;
        inner.next_event_number += events_len;
        let tx_hash = receipt.tx_hash;
        let conf = confirmation(receipt, inner.next_event_number).unwrap();

        for event in &conf.events {
            self.events_sender
                .send(SequencerEvent {
                    tx_hash,
                    event: event.clone(),
                })
                .ok();
        }

        trace!(events_len, "Transaction was accepted by the sequencer");

        inner.update_api_state().await; // TODO: we only want to do this when updated state from node?

        Ok(AcceptedTx {
            tx: baked_tx,
            tx_hash,
            confirmation: conf,
        })
    }

    async fn tx_status(
        &self,
        _tx_hash: &TxHash,
    ) -> anyhow::Result<
        TxStatus<<<Self::Spec as Spec>::Da as sov_modules_api::DaSpec>::TransactionId>,
    > {
        // At the time of writing, information in the DB is not stored in such a
        // way that facilitates random access to tx status information. That
        // means the sequencer only relies on the cache. FIXME(@neysofu).
        Ok(TxStatus::Unknown)
    }

    async fn submit_batch(
        &self,
        txs: Vec<FullyBakedTx>,
    ) -> anyhow::Result<Option<SubmitBatchReceipt>> {
        for tx in txs.iter() {
            self.accept_tx(tx.clone()).await.ok(); // FIXME(@neysofu): handle error.
        }

        let mut inner = self.inner.lock().await;

        if let Some(batch) = inner.produce_batch_if_possible().await? {
            inner
                .blob_sender
                .publish_batch_and_wait(batch)
                .await
                .map(Some)
        } else {
            Ok(None)
        }
    }
}

struct PreferredBatchToRestore {
    is_in_progress: bool,
    visible_slot_number_after_increase: VisibleSlotNumber,
    batch: WithCachedTxHashes<PreferredBatchData>,
}

/// Configuration for [`PreferredSequencer`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Eq, PartialEq, JsonSchema)]
pub struct PreferredSequencerConfig {
    /// The minimum fee that the preferred sequencer is willing to accept, denominated in rollup tokens. Defaults to zero.
    /// Sequencers should set this to a non-zero value if they wish to cover their DA costs.
    #[serde(default)]
    pub minimum_profit_per_tx: u128,
    /// The size of the Tokio channel used to stream events.
    ///
    /// Don't deviate from the default unless you know what you're doing.
    #[serde(default = "default_events_channel_size")]
    pub events_channel_size: usize,
    /// Optional. When present, Postgres will be used as a database instead of
    /// RocksDB.
    #[serde(default)]
    pub postgres_connection_string: Option<String>,
}

impl Default for PreferredSequencerConfig {
    fn default() -> Self {
        Self {
            minimum_profit_per_tx: 0,
            events_channel_size: default_events_channel_size(),
            postgres_connection_string: None,
        }
    }
}

fn default_events_channel_size() -> usize {
    100
}

#[async_trait]
impl<S, Rt, Da> SequenceNumberProvider for PreferredSequencer<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    async fn generate_sequence_number(&self, preferred_blob: &[u8]) -> anyhow::Result<u64> {
        self.inner
            .lock()
            .await
            .db
            .insert_proof_blob(preferred_blob.to_vec())
            .await
    }
}

#[derive(Debug)]
struct BackgroundTaskState<S: Spec> {
    handle: JoinHandle<(<<S as Spec>::Storage as Storage>::Root, ChangeSet)>,
    tx_sender: mpsc::Sender<FullyBakedTx>,
    result_receiver: mpsc::Receiver<Result<(TransactionReceipt<S>, TxChangeSet), RejectReason>>,
}

impl<S: Spec> BackgroundTaskState<S> {
    fn shutdown(self) -> JoinHandle<(<<S as Spec>::Storage as Storage>::Root, ChangeSet)> {
        // Must be dropped before the result receiver, or a deadlock happens.
        drop(self.tx_sender);
        self.handle
    }
}

#[serde_with::serde_as]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TxBody(#[serde_as(as = "serde_with::base64::Base64")] Vec<u8>);

/// Transaction confirmation data of [`PreferredSequencer`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(bound = "S: Spec, Rt: Runtime<S>")]
pub struct Confirmation<S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    events: Vec<RuntimeEventResponse<<Rt as RuntimeEventProcessor>::RuntimeEvent>>,
    receipt: ApiTxEffect<TxReceiptContents<S>>,
}

fn confirmation<S, Rt>(
    receipt: TransactionReceipt<S>,
    next_event_number: u64,
) -> anyhow::Result<Confirmation<S, Rt>>
where
    S: Spec,
    Rt: Runtime<S>,
{
    Ok(Confirmation {
        events: receipt
            .events
            .into_iter()
            .zip(next_event_number..)
            .map(|(event, number)| {
                <RuntimeEventResponse<<Rt as RuntimeEventProcessor>::RuntimeEvent>>::try_from((
                    number, event,
                ))
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
        receipt: receipt.receipt.into(),
    })
}

#[tracing::instrument(skip_all, level = "trace")]
async fn batches_to_process<S, Rt>(
    db: &mut PreferredSequencerDb<S, Rt>,
    info: &StateUpdateInfo<S::Storage>,
) -> anyhow::Result<Vec<PreferredBatchToRestore>>
where
    S: Spec,
    Rt: Runtime<S>,
{
    let blobs_to_apply = match db.subsequent_completed_blobs(info).await {
        Ok(b) => b,
        Err(err) => {
            error!(%err, "Database error while re-applying state changes. This is a critical error. Database integrity is intact, but the sequencer may momentarily provide outdated state and break soft-confirmations.");
            return Err(err);
        }
    };

    let first_sequence_number = blobs_to_apply.first().map(|b| b.sequence_number());

    trace!(
        blobs_count = blobs_to_apply.len(),
        first_sequence_number,
        last_sequence_number = blobs_to_apply.last().map(|b| b.sequence_number()),
        "Extracted blobs to apply from database"
    );

    let mut batches: Vec<_> = blobs_to_apply
        .into_iter()
        .filter_map(|blob| match blob {
            PreferredSequencerReadBlob::Batch(batch) => Some(PreferredBatchToRestore {
                is_in_progress: false,
                visible_slot_number_after_increase: batch.visible_slot_number_after_increase,
                batch: batch.into(),
            }),
            // TODO(https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/2063): Process proofs.
            // Note: once we start processing proofs in addition to batches,
            // we gotta make sure to order everything by sequence number as
            // proofs can have a sequence number that's greater than the
            // in-progress batch.
            _ => {
                trace!(
                    sequence_number = %blob.sequence_number(),
                    "Ignoring proof blob"
                );
                None
            }
        })
        .collect();

    if let Some(batch) = db.in_progress_batch_opt().await?.cloned() {
        batches.push(PreferredBatchToRestore {
            is_in_progress: true,
            visible_slot_number_after_increase: batch.visible_slot_number_after_increase,
            batch: batch.into(),
        });
    }

    Ok(batches)
}

fn next_visible_slot_number_increase<S: Spec>(
    checkpoint: &StateCheckpoint<S>,
    info: &StateUpdateInfo<S::Storage>,
    leave_space_for_next_batch: bool,
) -> Result<NonZero<u8>, SequencerNotReadyDetails> {
    trace!(?checkpoint, ?info, %leave_space_for_next_batch, "Calculating next visible slot number");

    let mut delta = info
        .latest_finalized_slot_number
        .checked_sub(checkpoint.current_visible_slot_number().get());

    if leave_space_for_next_batch {
        delta = delta.and_then(|x| x.checked_sub(1));
    }

    match delta.and_then(|delta| NonZero::new(delta.get().try_into().unwrap_or(u8::MAX))) {
        Some(delta) => Ok(delta),
        _ => Err(SequencerNotReadyDetails::WaitingOnDa {
            finalized_da_height: info.latest_finalized_slot_number.get(),
            needed_finalized_height: info
                .latest_finalized_slot_number
                .get()
                .checked_add(1)
                .expect(
                "Slot number overflow! This should be unreachable in the next few billion years",
            ),
        }),
    }
}

/// A helper function to allow recovering an associated consant from an *instance* of a type
/// when the type itself is unknown. This is useful when a function returns `impl Trait` and we
/// want to get an associated item from that trait implementation.
fn accepts_preferred_batches<B: BlobSelector>(_blob_selector: B) -> bool {
    B::ACCEPTS_PREFERRED_BATCHES
}
