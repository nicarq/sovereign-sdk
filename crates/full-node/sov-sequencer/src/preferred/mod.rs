//! See [`PreferredSequencer`].

mod async_batch;
mod batch_size_tracker;
mod block_executor;
mod db;
mod executor_events;
mod inner;
mod preferred_blob_sender;
mod replica_sync_task;
mod side_effects;
mod state_root_compute;
mod transaction_subscriptions;
mod update_state;
use std::boxed::Box;
use std::marker::PhantomData;
use std::num::NonZero;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::http::StatusCode;
use batch_size_tracker::BatchSizeTracker;
use db::postgres::PostgresBackend;
use db::rocksdb::RocksDbBackend;
use db::{
    PreferredSequencerDb, PreferredSequencerDbBackend, PreferredSequencerReadBatch,
    PreferredSequencerReadBlob,
};
use futures::Stream;
use inner::*;
use preferred_blob_sender::PreferredBlobSender;
use replica_sync_task::spawn_replica_sync_task;
use schemars::JsonSchema;
use serde_with::serde_as;
use side_effects::SideEffectsTask;
use sov_blob_sender::{new_blob_id, BlobExecutionStatus, BlobSender};
use sov_blob_storage::{PreferredBatchData, SequenceNumber};
use sov_db::ledger_db::LedgerDb;
use sov_modules_api::capabilities::{BlobSelector, RollupHeight, TransactionAuthenticator};
use sov_modules_api::macros::config_value;
use sov_modules_api::rest::utils::ErrorObject;
use sov_modules_api::rest::{ApiState, StateUpdateReceiver};
use sov_modules_api::{
    ApiTxEffect, FullyBakedTx, RejectReason, Runtime, RuntimeEventProcessor, RuntimeEventResponse,
    Spec, StateCheckpoint, StateUpdateInfo, VersionReader, VisibleSlotNumber, *,
};
use sov_rest_utils::errors::{database_error_500, sequencer_overloaded_503};
use sov_rollup_interface::common::SlotNumber;
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::node::DaSyncState;
use sov_rollup_interface::TxHash;
use state_root_compute::StateRootBackgroundTaskState;
use tokio::sync::mpsc::{self, Sender};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use transaction_subscriptions::TransactionCache;
use uuid::Uuid;

use crate::common::{
    error_not_fully_synced, generic_accept_tx_error, loop_send_tx_notifications, poll_state_update,
    AcceptedTx, Sequencer, SequencerEventStream, StateUpdateError, StateUpdateNotification,
    TxStatusBlobSenderHooks, WithCachedTxHashes,
};
use crate::metrics::{track_in_progress_batch_size, PreferredSequencerFetchBatchesToReplayMetrics};
use crate::preferred::block_executor::{
    RollupBlockExecutor, RollupBlockExecutorConfig, RollupBlockExecutorError,
};
use crate::preferred::db::DbEvent;
use crate::preferred::executor_events::ExecutorEventsSender;
use crate::preferred::preferred_blob_sender::create_blobs_to_send;
use crate::preferred::transaction_subscriptions::TxResultWriter;
use crate::rest_api::ApiAcceptedTx;
use crate::{
    ProofBlobSender, SequencerConfig, SequencerNotReadyDetails, TxStatus, TxStatusManager,
};

type VisibleSlotNumberIncrease = NonZero<u8>;

// Big infodump for the user that wouldmake the code hard to read if it were inline.
const RECOVERY_ERROR_MESSAGE_ON_NONE_STRATEGY: &str = "The preferred sequencer is too far behind, and the visible slot number has lagged more than the allowed deferred slots count. This means some non-preferred batches may have been included by the node, if there were any. If this happened, already provided soft confirmations may now no longer be valid. Because the recovery_strategy config was set to None, we are not attempting recovery at this point. You should either: a) delete everything from the preferred_sequencer database (thus annulling all currently pending soft confirmations), which will allow you to restart the sequencer fresh; or b) set the recovery_strategy config value to TryToSave, in which case all pending batches will be flushed to be executed on a best-effort basis. The latter may save some soft-confirmations if they have not been invalidated yet. However, IF a non-preferred batch has been included, AND some soft-confirmations have been invalidated by it, this will cause the sequencer to be penalised for every invalid batch; ensure your sequencer bond is sufficient to cover any penalties to be able to continue operating uninterrupted.";

/// Strategy for handling the scenario where the preferred sequencer finds itself close to or past
/// deferred_slots_count in the past, i.e. risking its soft confirmations being invalidated due to
/// the possibility of a non-preferred (deferred) batch having been included.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Eq, PartialEq, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub enum RecoveryStrategy {
    /// Do not attempt recovery, shutdown the sequencer instead. The user may attempt to resume
    /// operation either by swapping to TryToSave, or deleting everything from the preferred
    /// sequencer database (cancelling ALL pending soft confirmations!).
    None,
    /// Attempt to recover by flushing batches and catching up with the chain. Triggers a bit more
    /// conservatively to attempt to preserve soft confirmations (but if the sequencer was offline,
    /// this will likely make no difference). If some soft confirmations have indeed been
    /// invalidated, the sequencer will be penalized for every invalid batch!
    TryToSave,
}

<<<<<<< HEAD
=======
/// A inner sequencer struct containing state that requires synchronized access.
/// This struct accepts/rejects transactions, then hands them to the side effects task
/// to be persisted.
struct Inner<S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    latest_info: StateUpdateInfo<S::Storage>,

    config: SequencerConfig<S::Da, S::Address, PreferredSequencerConfig>,
    shutdown_receiver: watch::Receiver<()>,
    executor: RollupBlockExecutor<S, Rt>,
    batch_size_tracker: BatchSizeTracker,
    is_ready: Result<(), SequencerNotReadyDetails>,
    in_flight_blobs: Arc<AtomicUsize>,
    executor_events_sender: ExecutorEventsSender<S, Rt>,
    sequence_number_of_next_blob: SequenceNumber,
    /// A boolean that indicates whether the sequencer has finished its startup phase.
    /// We need this rather than relying on `SequencerNotReadyDetails::Startup` because that state
    /// can be overwritten when the node is resyncing.
    has_finished_startup: bool,
}

impl<S, Rt> Inner<S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    fn nb_of_concurrent_blob_submissions(&self) -> usize {
        self.in_flight_blobs.load(Ordering::Acquire)
    }

    pub async fn publish_proof_blob(&mut self, blob_id: BlobInternalId, data: Arc<[u8]>) {
        let sequence_number = self.get_and_inc_next_sequence_number();
        self.executor_events_sender
            .send(ExecutorEvent::PublishProofBlob(
                blob_id,
                data,
                sequence_number,
            ))
            .await;
    }

    async fn overwrite_next_sequence_number_for_recovery(
        &mut self,
        sequence_number: SequenceNumber,
    ) {
        info!(%sequence_number, "Overwriting next sequence number");
        self.sequence_number_of_next_blob = sequence_number;
        track_sequence_number(self.sequence_number_of_next_blob);
    }

    fn blob_sender_busy(&self) -> Option<usize> {
        let num_current_in_flight = self.nb_of_concurrent_blob_submissions();

        if num_current_in_flight > self.config.max_concurrent_blobs {
            Some(num_current_in_flight)
        } else {
            None
        }
    }

    fn node_root_hash(&self) -> anyhow::Result<<S::Storage as Storage>::Root> {
        self.latest_info
            .storage
            .get_root_hash(self.latest_info.slot_number)
    }

    fn current_height(&self) -> RollupHeight {
        self.executor.checkpoint.rollup_height_to_access()
    }

    /// Create a new batch, if possible. Errors here are expected, because it's not always possible to create a new batch due to transient DA issues.
    /// We can only create a new batch if we have a finalized slot available to use as our `visible_slot_number_after_increase`.
    #[tracing::instrument(skip_all, level = "trace")]
    async fn try_to_create_and_start_batch_if_none_in_progress(
        &mut self,
        stop_at_rollup_height: Option<RollupHeight>,
        leave_space_for_next_batch: bool,
    ) -> Result<(), BatchCreationError> {
        if self.executor.has_in_progress_batch() {
            return Ok(());
        }

        if let Some(height_to_stop_at) = stop_at_rollup_height {
            let current_height = self.current_height();
            if current_height >= height_to_stop_at {
                debug!("The sequencer is at stop height and tried to create a batch (aborted due to stop height).");
                return Err(BatchCreationError::PreferredSequencerAtStopHeight {
                    current_height,
                    height_to_stop_at,
                });
            }
        }

        if self.blob_sender_busy().is_some() {
            warn!("The blob sender is busy, no batch could be started at this time.");
            return Err(BatchCreationError::BlobSenderBusy);
        }

        let visible_increase = match next_visible_slot_number_increase(
            &self.executor.checkpoint,
            &self.latest_info,
            leave_space_for_next_batch,
            self.config
                .sequencer_kind_config
                .ideal_lag_behind_finalized_slot,
        ) {
            Ok(visible_increase) => visible_increase,
            Err(e) => {
                warn!(
                    "A batch was requested but the sequencer is not ready to produce one: {:?}",
                    e
                );
                return Err(BatchCreationError::NoFinalizedSlotAvailable);
            }
        };

        debug!(visible_increase, "No in-progress batch, starting a new one");
        let node_state_root = self
            .node_root_hash()
            .map_err(BatchCreationError::DatabaseError)?;
        let visible_slot_number_after_increase = self
            .executor
            .checkpoint
            .current_visible_slot_number()
            .advance(visible_increase.get().into());

        // DB operations handled by replica-aware db implementation
        let sequence_number = self.get_and_inc_next_sequence_number();

        let min_profit_per_tx = self.config.sequencer_kind_config.minimum_profit_per_tx;
        self.executor
            .start_rollup_block(
                visible_slot_number_after_increase,
                visible_increase,
                &node_state_root,
                min_profit_per_tx,
            )
            .await;
        self.executor_events_sender
            .send(ExecutorEvent::StartBatch {
                visible_slot_number_after_increase,
                visible_slots_to_advance: visible_increase,
                sequence_number,
                new_checkpoint: self
                    .executor
                    .checkpoint
                    .clone_with_empty_witness_dropping_temp_cache(),
            })
            .await;

        Ok(())
    }

    /// Creates and starts a batch for replicas using the exact visible slot parameters from the master
    #[tracing::instrument(skip_all, level = "trace")]
    pub(crate) async fn try_start_batch_with_parameters_from_master(
        &mut self,
        visible_slot_number_after_increase: VisibleSlotNumber,
        visible_slots_to_advance: NonZero<u8>,
    ) -> anyhow::Result<()> {
        if self.executor.has_in_progress_batch() {
            return Ok(());
        }

        // Calculate the correct visible_slots_to_advance for this replica based on its current state
        let current_visible_slot_number = self.executor.checkpoint.current_visible_slot_number();
        let replica_visible_slots_to_advance = visible_slot_number_after_increase.as_true()
            .checked_sub(current_visible_slot_number.as_true().get())
            .and_then(|diff| NonZero::new(diff.get().try_into().unwrap()))
            .ok_or_else(|| {
                error!(
                    current_visible_slot_number = %current_visible_slot_number,
                    target_visible_slot_number = %visible_slot_number_after_increase,
                    "Cannot calculate visible slots to advance for replica: target is not greater than current"
                );
                anyhow!("Invalid visible slot number progression for replica".to_string())
            })?;

        assert_eq!(
            visible_slots_to_advance,
            replica_visible_slots_to_advance,
            "Sanity check failed: replica visible_slots_to_advance calculation different from master."
        );

        let node_state_root = self.node_root_hash()?;
        let sequence_number = self.get_and_inc_next_sequence_number();

        let min_profit_per_tx = self.config.sequencer_kind_config.minimum_profit_per_tx;
        self.executor
            .start_rollup_block(
                visible_slot_number_after_increase,
                replica_visible_slots_to_advance,
                &node_state_root,
                min_profit_per_tx,
            )
            .await;

        self.executor_events_sender
            .send(ExecutorEvent::StartBatch {
                visible_slot_number_after_increase,
                visible_slots_to_advance,
                sequence_number,
                new_checkpoint: self
                    .executor
                    .checkpoint
                    .clone_with_empty_witness_dropping_temp_cache(),
            })
            .await;

        Ok(())
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn trigger_batch_production_if_convenient(
        &mut self,
        stop_at_rollup_height: Option<RollupHeight>,
    ) {
        if !self.config.automatic_batch_production {
            warn!("Skipping batch production due to settings");
            return;
        }

        // If we're lagging less than the ideal amount, it's not convenient to create a new batch so return early
        if is_lagging_less_than_ideal_amount(
            self.executor.checkpoint.current_visible_slot_number(),
            self.latest_info.latest_finalized_slot_number,
            self.config
                .sequencer_kind_config
                .ideal_lag_behind_finalized_slot,
        ) {
            return;
        }

        if let Err(e) = self
            .try_to_create_and_start_batch_if_none_in_progress(stop_at_rollup_height, true)
            .await
        {
            tracing::debug!(
                error = %e,
                "Unable to start new batch after successful state update."
            );
        }

        // We were unable to open a new batch (likely due to a lack of finalized
        // slots), so we're done.
        if !self.executor.has_in_progress_batch() {
            return;
        }

        // If the node is shutting down, we may not be able to terminate the batch. In that case, just return early.
        if self.shutdown_receiver.has_changed().unwrap_or(true) {
            info!(
                "The sequencer is shutting down. Exiting trigger_batch_production_if_convenient."
            );
            return;
        }

        self.close_current_batch().await;
    }

    /// Closes the current batch
    #[cfg(feature = "test-utils")]
    pub async fn force_close_current_batch(&mut self) -> anyhow::Result<()> {
        self.close_current_batch().await;
        Ok(())
    }

    fn next_sequence_number(&self) -> SequenceNumber {
        self.sequence_number_of_next_blob
    }

    fn get_and_inc_next_sequence_number(&mut self) -> SequenceNumber {
        let sequence_number = self.sequence_number_of_next_blob;
        self.sequence_number_of_next_blob = self
            .sequence_number_of_next_blob
            .checked_add(1)
            .expect("Sequence number overflow; this should be unreachable for a few billion years");
        track_sequence_number(self.sequence_number_of_next_blob);
        sequence_number
    }

    /// Closes the current batch.
    ///
    /// This should be called only when...
    /// 1. There's no more capacity to accept txs in the current batch.
    /// 2. We're absolutely sure we want to close the batch early even though we don't need to.
    ///
    /// Case 2 only happens when we've just finished updating the state *and* we have more than our ideal number of finalized slots available.
    #[tracing::instrument(skip_all, level = "trace")]
    async fn close_current_batch(&mut self) {
        // Terminate the batch.
        self.executor.end_rollup_block().await;
        self.batch_size_tracker = BatchSizeTracker::new(self.config.max_batch_size_bytes);
        let checkpoint = self
            .executor
            .checkpoint
            .clone_with_empty_witness_dropping_temp_cache();
        self.executor_events_sender
            .send(ExecutorEvent::CloseBatch(checkpoint))
            .await;
    }

    async fn prune_sequencer_db(&mut self) {
        let latest_state_info = &self.latest_info;
        let mut runtime = Rt::default();
        let next_sequence_number_according_to_node =
            get_next_sequence_number_according_to_node(latest_state_info, &mut runtime);

        sov_metrics::track_metrics(|tracker| {
            tracker.submit_inline(
                "sov_rollup_sequence_number_delta",
                format!(
                    "delta={}i",
                    (self.next_sequence_number() as i64)
                        - (next_sequence_number_according_to_node as i64)
                ),
            );
        });

        match latest_finalized_sequence_number(latest_state_info, &mut runtime) {
            Some(num) => {
                // TODO(@neysofu): somehow, if we prune too close to the latest
                // finalized sequence number, we get panics due to missing blobs
                // and inconsistent state. There is clearly something wrong with
                // the pruning height calculation height.
                if let Some(num) = num.checked_sub(100) {
                    self.executor_events_sender
                        .send(ExecutorEvent::PruneDb(num))
                        .await;
                }
            }
            None => {
                // Nothing to prune because there's no sequence number history.
            }
        }
    }

    async fn force_overwrite_state(
        &mut self,
        info: StateUpdateInfo<S::Storage>,
        new_executor: RollupBlockExecutor<S, Rt>,
    ) {
        tracing::trace!(?info, "Overwriting preferred sequencer internal state");

        // Replace known info
        self.latest_info = info.clone();

        // Replace executor state
        self.executor.replace_state(new_executor).await;

        // Replace API state
        let mut rt = Rt::default();
        let checkpoint = StateCheckpoint::new(info.storage.clone(), &rt.kernel());
        self.executor_events_sender
            .send(ExecutorEvent::ForceUpdateApiState(checkpoint))
            .await;
    }
}

>>>>>>> Prevent sequencer from creating batches past the stop_at_height parameter
/// A [`Sequencer`] with instant transaction confirmation.
pub struct PreferredSequencer<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    synchronized_state_updator: SequencerStateUpdator<S, Rt>,
    tx_status_manager: TxStatusManager<S::Da>,
    blobs_sender_channel: broadcast::Sender<BlobExecutionStatus<Da::Spec>>,
    api_state: ApiState<S>,
    da_sync_state: Arc<DaSyncState>,
    _runtime: PhantomData<(Rt, Da)>,
    config: SequencerConfig<S::Address, PreferredSequencerConfig>,
    da_address: <S::Da as DaSpec>::Address,
    block_executors_shutdown_notifier: Sender<()>,
    /// Unique node identifier used for leader election in replica failover scenarios. Generated on
    /// startup (and reset on restart).
    node_id: Uuid,
    is_master: AtomicBool,
    state_root_compute_task: StateRootBackgroundTaskState<S>,
    shutdown_receiver: watch::Receiver<()>,
    transaction_cache: TransactionCache<S, Rt>,
    // This ledgerdb is used specifically for REST API and websocket subscriptions.
    // The sequencer controls when it is updated to solve inconsistency issues,
    // See [`LedgerDb::with_shared_notifications`] for more details.
    api_ledger_db: LedgerDb,
    shutdown_sender: watch::Sender<()>,
    // Used to track which txs need to be ignored after the sequencer had downtime (in the sense of giving out 503s)
    tx_queue_id: Arc<AtomicU64>,
    stop_at_rollup_height: Option<RollupHeight>,
    /// The sender for state update notifications. Currently used only for testing.
    test_only_state_update_notification_sender: broadcast::Sender<StateUpdateNotification>,
}

impl<S, Rt, Da> PreferredSequencer<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    /// At the time of writing, the [`PreferredSequencer`] doesn't use
    /// the [`TxStatusManager`].
    ///
    /// The [`Sequencer`] itself already updates the
    /// [`TxStatusManager`] after all operations, so we'd only need it if we
    /// ever "drop" previously-accepted transactions. The whole point of the
    /// [`PreferredSequencer`] is that we *don't* do that.
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        da: Da,
        state_update_receiver: StateUpdateReceiver<S::Storage>,
        da_sync_state: Arc<DaSyncState>,
        storage_path: &Path,
        config: &SequencerConfig<S::Address, PreferredSequencerConfig>,
        ledger_db: LedgerDb,
        api_ledger_db: LedgerDb,
        shutdown_sender: watch::Sender<()>,
        stop_at_rollup_height: Option<RollupHeight>,
    ) -> anyhow::Result<(Arc<Self>, Vec<JoinHandle<()>>)> {
        let shutdown_receiver = shutdown_sender.subscribe();
        let latest_state_update = state_update_receiver.borrow().clone();
        let da_address = da.get_signer().await;
        debug!(
            ?latest_state_update,
            %da_address,
            "Instantiating the preferred sequencer"
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

        // Rocksdb sequencers always start as master, since there is no support for replication.
        // Postgres sequencers always start as a replica and attempt to register as master after
        // startup.
        let start_as_master = config
            .sequencer_kind_config
            .postgres_connection_string
            .is_none();

        let (block_executors_shutdown_notifier, block_executors_shutdown_rx) = mpsc::channel(1);

        let (blobs_sender_channel, _) =
            broadcast::channel(config.sequencer_kind_config.events_channel_size);

        // Generate node ID early so it can be used for database backend creation
        let node_id = Uuid::now_v7();

        let db_backend: Box<dyn PreferredSequencerDbBackend> =
            if let Some(postgres_connection_string) =
                &config.sequencer_kind_config.postgres_connection_string
            {
                Box::new(PostgresBackend::connect(postgres_connection_string, node_id).await?)
            } else {
                Box::new(RocksDbBackend::new(storage_path).await?)
            };
<<<<<<< HEAD
        let (db, latest_db_event_id, next_sequence_number, db_cache) =
            PreferredSequencerDb::<S, Rt>::new(
                db_backend,
                shutdown_sender.clone(),
                config.sequencer_kind_config.is_replica,
            )
            .await?;
=======
        let (db, latest_event_id, next_sequence_number) = PreferredSequencerDb::<S, Rt>::new(
            db_backend,
            shutdown_sender.clone(),
            start_as_master,
        )
        .await?;
>>>>>>> Add APIS and rename is_replica to is_master

        let mut handles = vec![];

        let completed_blobs = db_cache.all_completed_blobs();

        let blob_sender = {
            let blobs_to_send = if config.sequencer_kind_config.is_replica {
                Vec::new()
            } else {
                // It's possible that sov-blob-sender's DB might miss some blob data at
                // node startup due to:
                //  1. Disk failure (the sequencer can use Postgres so it's durable).
                //  2. DB corruption.
                //  3. Node crash at an inconvenient time.
                // Let's restore all missing blob data to make sure they land on the DA.
                create_blobs_to_send(completed_blobs)?
            };

            let (inner_blob_sender, blob_sender_handle) = BlobSender::new(
                da,
                ledger_db.clone(),
                storage_path,
                TxStatusBlobSenderHooks::new(tx_status_manager.clone()),
                shutdown_sender.clone(),
                Duration::from_secs(config.blob_processing_timeout_secs),
                Some(blobs_sender_channel.clone()),
                blobs_to_send,
            )
            .await?;

            handles.push(blob_sender_handle);
<<<<<<< HEAD
            PreferredBlobSender::from((inner_blob_sender, config.sequencer_kind_config.is_replica))
=======

            let mut blob_sender = PreferredBlobSender::from((inner, start_as_master));

            // It's possible that sov-blob-sender's DB might miss some blob data at
            // node startup due to:
            //  1. Disk failure (the sequencer can use Postgres so it's durable).
            //  2. DB corruption.
            //  3. Node crash at an inconvenient time.
            // Let's restore all missing blob data to make sure they land on the DA.
            blob_sender.publish_blobs(completed_blobs).await?;

            blob_sender
>>>>>>> Add APIS and rename is_replica to is_master
        };

        let (state_root_compute_handle, state_root_compute_task) =
            StateRootBackgroundTaskState::create(
                block_executors_shutdown_rx,
                !config
                    .sequencer_kind_config
                    .disable_state_root_consistency_checks,
            );
        handles.push(state_root_compute_handle);

        // TODO: Rename events_channel_size to transaction_channel_size
        let cached_txs = TransactionCache::new(
            api_ledger_db.clone(),
            latest_state_update.next_tx_number,
            config.sequencer_kind_config.events_channel_size,
        );

        let (executor_events_sender, executor_events_receiver) =
            ExecutorEventsSender::new(shutdown_sender.clone(), db_cache);
        let in_flight_blobs = blob_sender.nb_of_in_flight_blobs_handle();

        // Here we need to mutliply by 1000 to convert from millis to micros.
        let batch_execution_time_limit_micros = config
            .sequencer_kind_config
            .batch_execution_time_limit_millis
            * 1000;

        let rollup_exec_config = RollupBlockExecutorConfig {
            config: config.clone(),
            da_address: da_address.clone(),
            shutdown_notifier: block_executors_shutdown_notifier.clone(),
            state_root_request_sender: state_root_compute_task.request_sender.clone(),
            shutdown_receiver: shutdown_receiver.clone(),
            shutdown_sender: shutdown_sender.clone(),
            startup_transaction_cache_writer: None, // The main executor must *not* write to the tx cache. That's handled by the side effects task
        };

        let tx_queue_id = Arc::new(AtomicU64::new(0));
        let (synchronized_state, synchronized_state_updator) = create(
            latest_state_update.clone(),
            tx_queue_id.clone(),
            batch_execution_time_limit_micros,
            config.clone(),
            shutdown_receiver.clone(),
            shutdown_sender.clone(),
            rollup_exec_config,
            executor_events_sender,
            next_sequence_number,
            in_flight_blobs,
<<<<<<< HEAD
            stop_at_rollup_height,
        );

        let synchronized_state_task = synchronized_state.start().await;
        handles.push(synchronized_state_task);
=======
            batch_size_tracker: BatchSizeTracker::new(config.max_batch_size_bytes),
            is_ready: Err(SequencerNotReadyDetails::Startup),
        };
>>>>>>> Add APIS and rename is_replica to is_master

        let side_effects_task = SideEffectsTask {
            checkpoint_sender,
            blob_sender,
            executor_events_receiver,
            db,
            shutdown_sender: shutdown_sender.clone(),
            transaction_cache: cached_txs.write_handle(),
        }
        .spawn();
        handles.push(side_effects_task);

        let seq = Arc::new(PreferredSequencer {
            synchronized_state_updator,
            tx_status_manager: tx_status_manager.clone(),
            transaction_cache: cached_txs,
            blobs_sender_channel,
            da_sync_state,
            api_state,
            _runtime: PhantomData,
            block_executors_shutdown_notifier,
            config: config.clone(),
            node_id,
            is_master: AtomicBool::new(start_as_master),
            state_root_compute_task,
            shutdown_receiver: shutdown_receiver.clone(),
            api_ledger_db,
            shutdown_sender,
            tx_queue_id,
            stop_at_rollup_height,
            test_only_state_update_notification_sender: broadcast::channel(100).0,
            da_address,
        });

        // Launch replica sync task (only IF postgres is configured). Rocksdb sequencers have no
        // replication functionality.
        // This will block until the currently stored batches in the DB are replayed onto the
        // state, then yield when it switches to processing postgres events live.
        // This is necessary to prevent conflicts with the update_state task.
<<<<<<< HEAD
        handles.push(
            spawn_replica_sync_task(
                seq.clone(),
                shutdown_receiver.clone(),
                latest_state_update.clone(),
                latest_db_event_id,
            )
            .await,
        );
=======
        if let Some(connection_string) = &config.sequencer_kind_config.postgres_connection_string {
            handles.push(
                spawn_replica_sync_task(
                    seq.clone(),
                    shutdown_receiver.clone(),
                    latest_state_update.clone(),
                    connection_string.clone(),
                    latest_event_id,
                )
                .await,
            );
        }
>>>>>>> Fix tests that use rocksdb
        handles.push(tokio::spawn({
            update_state_task(
                seq.clone(),
                state_update_receiver.clone(),
                shutdown_receiver.clone(),
            )
        }));
        handles.push(tokio::spawn({
            let ledger_db = ledger_db.clone();
            let seq = seq.clone();
            let shutdown_rx = shutdown_receiver.clone();
            async move {
                loop_send_tx_notifications::<S, Rt>(
                    state_update_receiver,
                    shutdown_rx,
                    &ledger_db,
                    seq.tx_status_manager(),
                )
                .await;
            }
        }));

        Ok((seq, handles))
    }

<<<<<<< HEAD
=======
    #[tracing::instrument(skip_all, level = "debug")]
    pub(crate) async fn lock_inner(&self) -> MutexGuard<Inner<S, Rt>> {
        self.inner.lock().await
    }

    fn create_new_executor_for_replay(
        &self,
        info: &StateUpdateInfo<S::Storage>,
    ) -> RollupBlockExecutor<S, Rt> {
        RollupBlockExecutor::<_, Rt>::new(
            info,
            None, // we don't send events during replay or recovery
            None, // we don't send transactions during replay or recovery
            self.config.clone(),
            self.block_executors_shutdown_notifier.clone(),
            self.state_root_compute_task.request_sender.clone(),
            self.shutdown_receiver.clone(),
            self.shutdown_sender.clone(),
            self.cached_events.clone(),
        )
    }

    async fn check_readiness(
        &self,
        inner: &Inner<S, Rt>,
        max_concurrent_blobs: usize,
        height_to_stop_at: Option<RollupHeight>,
    ) -> Result<(), SequencerNotReadyDetails> {
        if !self.is_master().await {
            return Err(SequencerNotReadyDetails::ReplicaMode);
        }

        // We cannot accept transactions until the latest finalized slot number
        // is AT LEAST 1. Meaning, as long as we're stuck at genesis, we can't
        // accept any transactions.
        if inner.latest_info.latest_finalized_slot_number == SlotNumber::GENESIS {
            tracing::error!("Timed out while waiting for the node to progress beyond genesis. The sequencer can't accept transactions until that happens");
            return Err(SequencerNotReadyDetails::WaitingOnDa {
                finalized_slot_number: SlotNumber::GENESIS,
                needed_finalized_slot_number: SlotNumber::new(1),
            });
        }

        if let Some(nb_of_blobs_in_flight) = inner.blob_sender_busy() {
            return Err(SequencerNotReadyDetails::WaitingOnBlobSender {
                max_concurrent_blobs,
                nb_of_blobs_in_flight,
            });
        }

        if let Some(height_to_stop_at) = height_to_stop_at {
            let current_height = inner.current_height();
            if current_height > height_to_stop_at {
                return Err(SequencerNotReadyDetails::PreferredSequencerAtStopHeight {
                    current_height,
                    height_to_stop_at,
                });
            }
        }

        inner.is_ready.as_ref().map_err(|details| details.clone())?;
        Ok(())
    }

    fn current_visible_slot_number_according_to_node(
        &self,
        info: &StateUpdateInfo<S::Storage>,
    ) -> SlotNumber {
        let mut rt = Rt::default();
        let node_checkpoint = StateCheckpoint::new(info.storage.clone(), &rt.kernel());
        node_checkpoint.current_visible_slot_number().as_true()
    }

    async fn trigger_recovery(&self, info: &StateUpdateInfo<S::Storage>) {
        let mut inner = self.lock_inner().await;
        inner.is_ready = Err(SequencerNotReadyDetails::PreferredSequencerRecovering);
        let next_sequence_number_according_to_node =
            get_next_sequence_number_according_to_node(info, &mut Rt::default());
        inner
            .executor_events_sender
            .send(ExecutorEvent::EnterRecoveryMode {
                recovery_strategy: self.config.sequencer_kind_config.recovery_strategy.clone(),
                next_sequence_number_according_to_node,
            })
            .await;
        info!(?info, current_visible_slot_number = %self.current_visible_slot_number_according_to_node(info), "Beginning sequencer recovery");
    }

>>>>>>> fix rejection in accept_tx
    /// Returns a range to allow hysteresis during catchup. The first (lower) value will be the
    /// minimum to be considered successfully recovered, the second (upper) value will be the
    /// target.
    fn catchup_batches_to_send(&self, info: &StateUpdateInfo<S::Storage>) -> (u64, u64) {
        let current_visible_slot_number =
            current_visible_slot_number_according_to_node::<S, Rt>(info);
        let raw_catchup_delta = info
            .latest_finalized_slot_number
            .saturating_delta(current_visible_slot_number);
        let increase_per_batch = config_value!("MAX_VISIBLE_HEIGHT_INCREASE_PER_SLOT");

        let (maximum_delta, minimum_delta) = Self::slot_count_delta_acceptable_upper_bound_range(
            self.config.max_allowed_node_distance_behind,
        );
        tracing::debug!(deferred_slots_count = raw_max_deferred_slots_delay(self.config.max_allowed_node_distance_behind), maximum_delta, minimum_delta, current_catchup_delta = raw_catchup_delta, %current_visible_slot_number, increase_per_batch, "Calculating amount of batches to send");
        (
            raw_catchup_delta
                .saturating_sub(maximum_delta)
                .div_ceil(increase_per_batch),
            raw_catchup_delta
                .saturating_sub(minimum_delta)
                .div_ceil(increase_per_batch),
        )
    }

    async fn recover_and_catch_up(
        &self,
        state_update_receiver: &mut StateUpdateReceiver<S::Storage>,
        shutdown_receiver: &watch::Receiver<()>,
        mut info: StateUpdateInfo<S::Storage>,
    ) -> anyhow::Result<()> {
<<<<<<< HEAD
=======
        if !self.is_master().await {
            // Replicas don't run recovery. We let the main sequencer run catchup. If we fail-over
            // midway, update_state() will automatically re-trigger recovery on this instance if
            // necessary - if the previous master already recovered enough then we'll just continue
            // operating.
            //
            // TODO: we do need to overwrite our state with the node's. Since recovery is expected
            // to be very rare, and if it does happen that means the rollup has already had
            // downtime and will already have had lost soft-confirmations, for now we'll require
            // the user to manually reset replicas.
            // To implement this properly we'd need to make sure we're 100% synced with the master
            // on exactly when to stop overwriting from the node and start applying new
            // transactions again. Probably by watching the `txs` table, so shouldn't be hard, but
            // not trivial enough to implement it on the spot.
            error!("We have encountered recovery conditions, but this is a replica sequencer. Recovery is currently unsupported for replicas. Please run a single master instance of the sequencer to restore the rollup to normal functionality. Wait for the rollup to be fully recovered, and then restart any replicas.");
            exit_rollup(&self.shutdown_sender).await;
            unreachable!();
        }

>>>>>>> Add APIS and rename is_replica to is_master
        let mut rt = Rt::default();

        loop {
            let (min_batches_to_send, max_batches_to_send) = self.catchup_batches_to_send(&info);
            if min_batches_to_send == 0 {
                tracing::info!(
                    min_batches_to_send,
                    max_batches_to_send,
                    "Recovery: no need to send any more batches!"
                );
                break;
            }
            tracing::info!(min_batches_to_send, max_batches_to_send, "Recovery: sending max_batches_to_send empty catchup batches to bump the visible_slot_number");

            // 1. Dump our catchup batches once every DA block to fast-forward the
            //    visible_slot_number
            for i in 1..=max_batches_to_send {
                let start_height = self.da_sync_state.target_da_height.load(Ordering::Relaxed);
                loop {
                    let new_height = self.da_sync_state.target_da_height.load(Ordering::Relaxed);
                    if new_height > start_height {
                        break;
                    } else {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
                if i % 10 == 0 {
                    tracing::info!(number = %i, min = %min_batches_to_send, max = %max_batches_to_send, "Sending catchup batch");
                } else {
                    tracing::debug!(number = %i, min = %min_batches_to_send, max = %max_batches_to_send, "Sending catchup batch");
                }
<<<<<<< HEAD

                self.synchronized_state_updator
                    .trigger_batch_production_if_convenient_msg(
                        "recover_and_catch_up:dump_catchup_batches",
                    )
=======
                // We don't run force_overwrite_state() here.
                // This is mostly fine, mainly the API state will be out of date until we've
                // finished sending our batches.
                // Adding parallel state update handling is not worth the complexity right now.
                inner
                    .trigger_batch_production_if_convenient(self.stop_at_rollup_height)
>>>>>>> Prevent sequencer from creating batches past the stop_at_height parameter
                    .await;
            }

            // 2. Wait for node to catch up to our sequence number
            let target_sequence_number = self
                .synchronized_state_updator
                .next_sequence_number_msg("recover_and_catch_up:get_next_sequence_number")
                .await;

            tracing::info!(target_sequence_number, "Recovery: catchup batches sent; sequencer will now wait for the node to process them. We will then re-evaluate if we need to catch up again (if there are so many batches that by the time the node catches up we need to bump the visible_slot_number some more).");

            loop {
                let next_sequence_number_according_to_node =
                    get_next_sequence_number_according_to_node(&info, &mut rt);
                tracing::debug!(
                    next_sequence_number_according_to_node,
                    target_sequence_number,
                    "Recovery: waiting for the node to process sequencer's catchup batches..."
                );
                if next_sequence_number_according_to_node >= target_sequence_number {
                    tracing::info!("Node sequence number caught up to our recovery batches. The sequencer may have finished recovery, or we may need to send another round of batches if catching up this far took too long");
                    break;
                }

                info = poll_state_update::<S>(
                    state_update_receiver,
                    shutdown_receiver,
                    "update_state_task",
                )
                .await?;

                let rollup_exec_config = self.create_bloc_exec_config_for_recovery();
                self.synchronized_state_updator
                    .flush_tx_cache_msg(
                        self.api_ledger_db.clone(),
                        self.transaction_cache.write_handle(),
                        info.clone(),
                        rollup_exec_config,
                        "recover_and_catch_up:overwrite_executor",
                    )
                    .await;
            }
        }

        info!(
            ?info,
            "Sequencer exiting recovery and resuming normal operation."
        );
        Ok(())
    }

    // Creates a new executor config for recovery. This must *not* be called to create executors
    // under other circumstances, since it causes side effects on the transaction cache.
    //
    // If you need an executor for normal "replay" use a different constructor which does
    // not pass a transaction cache writer.
    fn create_bloc_exec_config_for_recovery(&self) -> RollupBlockExecutorConfig<S, Rt> {
        RollupBlockExecutorConfig {
            config: self.config.clone(),
            da_address: self.da_address.clone(),
            shutdown_notifier: self.block_executors_shutdown_notifier.clone(),
            state_root_request_sender: self.state_root_compute_task.request_sender.clone(),
            shutdown_receiver: self.shutdown_receiver.clone(),
            shutdown_sender: self.shutdown_sender.clone(),
            startup_transaction_cache_writer: Some(self.transaction_cache.write_handle()), // update the tx cache as we go
        }
    }

    /// How far to catch back up if we need to recover/fast-forward due to being too close to (or
    /// past) slot_count_delay_acceptable_lower_bound.
    /// Returns a range to allow hysteresis during catchup. The first (lower) value will be the
    /// minimum to be considered successfully recovered, the second (upper) value will be the
    /// target.
    fn slot_count_delta_acceptable_upper_bound_range(
        max_allowed_node_distance_behind: u64,
    ) -> (u64, u64) {
        // TODO: check this at compile time (#3041)
        const OVERFLOW_ERROR_STR: &str = "Overflow calculating deferred slots count range. In the future this should be handled better, but the config value should never be set high enough to cause this.";
        let raw_max_delay = raw_max_deferred_slots_delay(max_allowed_node_distance_behind);
        (
            // TODO: consider making these percentages a sequencer config value
            raw_max_delay.checked_mul(5).expect(OVERFLOW_ERROR_STR) / 10, // 50% of max delay
            raw_max_delay.checked_mul(4).expect(OVERFLOW_ERROR_STR) / 10, // 40% of max delay
        )
    }

    async fn wait_for_node_resync(
        &self,
        state_update_receiver: &mut StateUpdateReceiver<S::Storage>,
        shutdown_receiver: &watch::Receiver<()>,
        distance_to_tip: u64,
        current_info: StateUpdateInfo<S::Storage>,
    ) -> anyhow::Result<()> {
        let mut info = current_info;

        loop {
            let is_synced = self.da_sync_state.status().distance() <= distance_to_tip;

            self.synchronized_state_updator
                .wait_for_node_resync_msg(
                    self.api_ledger_db.clone(),
                    self.transaction_cache.write_handle(),
                    info,
                    self.da_sync_state.clone(),
                    "wait_for_node_resync",
                )
                .await;

            // Exit after processing if we're synced
            if is_synced {
                break;
            }

            // Else, poll a state update for the next iteration
            info = poll_state_update::<S>(
                state_update_receiver,
                shutdown_receiver,
                "update_state_task",
            )
            .await?;
        }
        Ok(())
    }

    async fn wait_for_node_resync_with_allowed_slack(
        &self,
        state_update_receiver: &mut StateUpdateReceiver<S::Storage>,
        shutdown_receiver: &watch::Receiver<()>,
        current_info: StateUpdateInfo<S::Storage>,
    ) -> anyhow::Result<()> {
        self.wait_for_node_resync(
            state_update_receiver,
            shutdown_receiver,
            // Catch up a bit extra to avoid immediately triggering another resync
            self.config.max_allowed_node_distance_behind.div_ceil(2),
            current_info,
        )
        .await
    }

    async fn wait_for_node_resync_to_tip(
        &self,
        state_update_receiver: &mut StateUpdateReceiver<S::Storage>,
        shutdown_receiver: &watch::Receiver<()>,
        current_info: StateUpdateInfo<S::Storage>,
    ) -> anyhow::Result<()> {
        self.wait_for_node_resync(state_update_receiver, shutdown_receiver, 1, current_info)
            .await
    }
}

<<<<<<< HEAD
<<<<<<< HEAD
pub(crate) fn slot_count_delta_acceptable_lower_bound(
    max_allowed_node_distance_behind: u64,
) -> u64 {
    // TODO: check this at compile time (#3041)
    const OVERFLOW_ERROR_STR: &str = "Overflow calculating deferred slots count range. In the future this should be handled better, but the config value should never be set high enough to cause this.";

    // Take 90% of the value to conservatively account for update_state not being called every
    // slot.
    // This will give us a better chance of successfully saving our soft-confirmations
    // if we catch it in time.
    // TODO: consider making this a sequencer config value
    raw_max_deferred_slots_delay(max_allowed_node_distance_behind)
        .checked_mul(9)
        .expect(OVERFLOW_ERROR_STR)
        / 10
}
=======
    async fn is_replica(&self) -> anyhow::Result<bool> {
        Ok(self.is_replica.load(Ordering::Acquire))
    }

=======
>>>>>>> Add APIS and rename is_replica to is_master
    /// Update the replica status (used during failover)
    pub async fn set_is_master(&self, is_master: bool) {
        // Sanity check - this should never be called for rocksdb sequencers
        if self
            .config
            .sequencer_kind_config
            .postgres_connection_string
            .is_none()
        {
            error!("The sequencer is running on rocksdb yet its replication status attempted to be changed. Rocksdb has no support for replication or failover; this is a bug, please report it.");
            exit_rollup(&self.shutdown_sender).await;
            unreachable!();
        }
        self.is_master.store(is_master, Ordering::Release);

        let inner = self.lock_inner().await;
        let next_sequence_number_according_to_node =
            get_next_sequence_number_according_to_node(&inner.latest_info, &mut Rt::default());
        inner
            .executor_events_sender
            .send(ExecutorEvent::UpdateMasterStatus {
                is_master,
                next_sequence_number_according_to_node,
            })
            .await;
    }

    /// Closes the current batch if it is nearly full (by gas limit) or has reached the target batch execution time.
    async fn close_batch_if_nearly_full(
        &self,
        inner: &mut Inner<S, Rt>,
        remaining_slot_gas: &<S as GasSpec>::Gas,
    ) {
        // Check if we're close to the gas limit and close the batch if we are.
        let mut comfortable_gas_limit = <S as GasSpec>::initial_gas_limit();
        comfortable_gas_limit
            .scalar_division(COMFORTABLE_GAS_LIMIT_DIVISOR)
            .checked_scalar_product(COMFORTABLE_GAS_LIMIT_MULTIPLIER)
            .unwrap_or_else(|| {
                panic!(
                    "Cannot overflow after dividing by {COMFORTABLE_GAS_LIMIT_DIVISOR} and multiplying by {COMFORTABLE_GAS_LIMIT_MULTIPLIER}",
                )
            });
        let close_to_gas_limit = remaining_slot_gas.dim_is_less_or_eq(&comfortable_gas_limit);
        if close_to_gas_limit {
            tracing::debug!(%comfortable_gas_limit, %remaining_slot_gas, "Closing and publishing current batch because we're close to the gas limit");
            inner.close_current_batch().await;
        }

        // Here we need to mutliply by 1000 to convert from millis to micros.
        let batch_execution_time_limit_micros = self
            .config
            .sequencer_kind_config
            .batch_execution_time_limit_millis
            * 1000;

        let current_batch_execution_time_micros =
            inner.batch_size_tracker.batch_execution_time_micros;

        if current_batch_execution_time_micros > batch_execution_time_limit_micros {
            tracing::debug!(%batch_execution_time_limit_micros, %current_batch_execution_time_micros, "Closing and publishing current batch because we've reached the batch execution time cap");
            inner.close_current_batch().await;
        } else {
            tracing::trace!(%batch_execution_time_limit_micros, %current_batch_execution_time_micros, "Batch execution time is within comfortable range, not closing batch");
        }
    }
>>>>>>> store is_replica

fn raw_max_deferred_slots_delay(max_allowed_node_distance_behind: u64) -> u64 {
    // TODO: there should be a DA config for added slack to account for DA inclusion delay here as well
    sov_blob_storage::config_deferred_slots_count()
        // Subtract one because node always force-increments visible slot number once it reaches
        // deferred_slots_count, so the delta will always be 1 below it during update_state
        .checked_sub(1)
        .expect("config_deferred_slots_count cannot be less than 1")
        // Subtract the max node delay because we know the node could be up to this far behind
        // (if it was further, we'd have triggered a resync). So the slot_number we will see
        // might be up to this far behind what it would be at the DA tip
        .checked_sub(max_allowed_node_distance_behind)
        .expect("config_deferred_slots_count cannot be lower than max_allowed_node_distance_behind")
}

async fn update_state_task<S, Rt, Da>(
    seq: Arc<PreferredSequencer<S, Rt, Da>>,
    mut state_update_receiver: StateUpdateReceiver<S::Storage>,
    shutdown_receiver: watch::Receiver<()>,
) where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    loop {
        if let Err(e) =
            update_state_task_inner(seq.clone(), &mut state_update_receiver, &shutdown_receiver)
                .await
        {
            // Thrown when polling for state updates is aborted due to a shutdown signal. Don't
            // re-send a second signal: we're already shutting down.
            if let Some(StateUpdateError::Shutdown) = e.downcast_ref::<StateUpdateError>() {
                info!("Received shutdown signal in update_state task, exiting gracefully.");
                return;
            }

            // For any other error, trigger a shut down
            error!("Error in preferred sequencer update state task: {e:?}. Shutting down rollup.");
            exit_rollup(&seq.shutdown_sender).await;
        }
    }
}

fn current_visible_slot_number_according_to_node<S: Spec, Rt: Runtime<S>>(
    info: &StateUpdateInfo<S::Storage>,
) -> SlotNumber {
    let mut rt = Rt::default();
    let node_checkpoint = StateCheckpoint::new(info.storage.clone(), &rt.kernel());
    node_checkpoint.current_visible_slot_number().as_true()
}

#[derive(Debug)]
pub(crate) enum PreferredSeqOperation {
    // Something has gone terribly wrong, and I don't see a way for us
    // to meaningfully recover without nuking the sequencer DB.
    Unreachable,
    // We found a preferred batch of which we have no memory very close
    // to the chain tip.
    WaitForNodeResyncToTip,
    // The node is lagging behind the chain tip. Pause the sequencer (if
    // it wasn't already paused), and wait for the node to catch up.
    WaitForNodeResyncWithAllowedSlack,
    // We are either dangerously close to hitting the
    // `deferred_slots_count` threshold or we've hit it already. Our
    // soft-confirmations might easily get invalidated.
    RecoverAndCatchUp,
    // This is by far the most common scenario, i.e. a nominal
    // `update_state` call during sequencer execution with no unusual
    // conditions.
    ReplaySoftConfirmationsOnTopOfNodeState(bool, Duration),
}

pub(crate) async fn update_api_ledger<S: Spec, Rt: Runtime<S>>(
    api_ledger_db: &LedgerDb,
    transaction_cache: TxResultWriter<S, Rt>,
    info: &StateUpdateInfo<S::Storage>,
) {
    api_ledger_db.replace_reader(info.ledger_reader.clone());
    api_ledger_db.send_notifications_for_slot(info.slot_number);
    prune_transactions_cache(info.next_tx_number, transaction_cache).await;
}

#[tracing::instrument(skip_all, level = "debug")]
async fn update_state_task_inner<S, Rt, Da>(
    seq: Arc<PreferredSequencer<S, Rt, Da>>,
    state_update_receiver: &mut StateUpdateReceiver<S::Storage>,
    shutdown_receiver: &watch::Receiver<()>,
) -> anyhow::Result<()>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    let info =
        poll_state_update::<S>(state_update_receiver, shutdown_receiver, "update_state").await?;
    if cfg!(debug_assertions) {
        let skip_flag = std::env::var("SOV_TEST_PAUSE_SEQUENCER_UPDATE_STATE");
        if skip_flag == Ok("1".to_string()) {
            tracing::warn!("skipping state update due to env var flag");
            return Ok(());
        }
    }
    let finalized_slot_number = info.latest_finalized_slot_number;
    let slot_number = info.slot_number;

    let mut rt = Rt::default();
    let timer_start = std::time::Instant::now();

    let next_sequence_number_according_to_node =
        get_next_sequence_number_according_to_node(&info, &mut rt);

    let recovery_rollup_exec_config = seq.create_bloc_exec_config_for_recovery();

    let operation = seq
        .synchronized_state_updator
        .sequencer_conditions_msg(
            &info,
            seq.da_sync_state.clone(),
            next_sequence_number_according_to_node,
            recovery_rollup_exec_config,
            "update_state::fetch_initial_batches_to_replay",
        )
        .await;

    match operation {
        PreferredSeqOperation::Unreachable => {
            panic!("The node has a higher sequence number than the sequencer, but the sequencer has some batches that it must replay (i.e. we're not just re-indexing a chain starting from an empty sequencer DB). This is an unusual scenario. It could mean you're running a competing preferred sequencer (which is not allowed!), or your sequencer DB data is corrupted... or it's just a bug. Please report it. You might attempt to recover by deleting the entire sequencer DB.")
        }

        PreferredSeqOperation::WaitForNodeResyncToTip => {
            seq.wait_for_node_resync_to_tip(state_update_receiver, shutdown_receiver, info)
                .await?;
        }

        PreferredSeqOperation::WaitForNodeResyncWithAllowedSlack => {
            seq.wait_for_node_resync_with_allowed_slack(
                state_update_receiver,
                shutdown_receiver,
                info,
            )
            .await?;
        }
        PreferredSeqOperation::RecoverAndCatchUp => {
            seq.recover_and_catch_up(state_update_receiver, shutdown_receiver, info)
                .await?;
        }

        PreferredSeqOperation::ReplaySoftConfirmationsOnTopOfNodeState(
            is_startup,
            time_spent_fetching_batches,
        ) => {
            seq.replay_soft_confirmations_on_top_of_node_state(
                info,
                timer_start,
                is_startup,
                time_spent_fetching_batches,
            )
            .await?;
        }
    }

    // Send a state update notification (for testing. Note that we've already released the lock at this point, so there should be no performance impact)
    // but updates are not strictly guaranteed to be delivered in order. We discard errors because we don't care if there are no subscribers.
    let _ = seq
        .test_only_state_update_notification_sender
        .send(StateUpdateNotification {
            slot_number,
            finalized_slot_number,
        });

    Ok(())
}

#[async_trait]
impl<S, Rt, Da> Sequencer for PreferredSequencer<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    type Confirmation = Confirmation<S, Rt>;
    type Spec = S;
    type Rt = Rt;
    type Da = Da;

    async fn list_events(
        &self,
        event_nums: std::ops::Range<u64>,
    ) -> Result<
        Vec<RuntimeEventResponse<<Self::Rt as RuntimeEventProcessor>::RuntimeEvent>>,
        anyhow::Error,
    > {
        let num_events = event_nums.end - event_nums.start;
        trace!(events_len = num_events, "listing events");

        let events = self.transaction_cache.list_events(event_nums).await?;

        trace!(result_len = events.len(), "retrieved events");

        Ok(events)
    }

    async fn is_ready(&self) -> Result<(), SequencerNotReadyDetails> {
        // We don't actually care about the `inner`, we just want to reuse the
        // same logic.
<<<<<<< HEAD
        self.synchronized_state_updator
            .check_readiness_msg(
                self.config.max_concurrent_blobs,
                self.stop_at_rollup_height,
                "check_readiness",
            )
            .await
            .map(|_| ())
=======
        let inner = self.inner.lock().await;
        self.check_readiness(
            &inner,
            self.config.max_concurrent_blobs,
            self.stop_at_rollup_height,
        )
        .await
        .map(|_| ())
>>>>>>> fix rejection in accept_tx
    }

    async fn is_master(&self) -> bool {
        self.is_master.load(Ordering::Acquire)
    }

    fn node_id(&self) -> Uuid {
        self.node_id
    }

    fn api_state(&self) -> ApiState<Self::Spec> {
        self.api_state.clone()
    }

    #[cfg(feature = "test-utils")]
    async fn force_close_current_batch(&self) {
        self.synchronized_state_updator
            .force_close_current_batch_msg("force_close_current_batch")
            .await;
    }

    #[cfg(feature = "test-utils")]
    async fn subscribe_state_updates_unstable(
        &self,
    ) -> Option<broadcast::Receiver<StateUpdateNotification>> {
        Some(self.test_only_state_update_notification_sender.subscribe())
    }

    fn tx_status_manager(&self) -> &TxStatusManager<<Self::Spec as Spec>::Da> {
        &self.tx_status_manager
    }

    async fn subscribe_events(&self) -> Option<SequencerEventStream<Self::Rt>> {
        use futures::StreamExt;

        let tx_stream = self.transaction_cache.subscribe();

        let event_stream: SequencerEventStream<Self::Rt> =
            Box::pin(tx_stream.flat_map(|tx| match tx {
                Ok(tx) => Box::pin(futures::stream::iter(
                    tx.confirmation.events.into_iter().map(Ok),
                )),
                Err(e) => {
                    let output: SequencerEventStream<Self::Rt> =
                        Box::pin(futures::stream::once(async { Err(e) }));
                    output
                }
            }));
        Some(event_stream)
    }

    async fn get_tx(
        &self,
        tx_hash: TxHash,
    ) -> anyhow::Result<Option<AcceptedTx<Self::Confirmation>>> {
        self.transaction_cache.get_tx_by_hash(tx_hash).await
    }

    async fn subscribe_transactions(
        &self,
        starting_from: Option<u64>,
    ) -> Option<
        anyhow::Result<
            Pin<Box<dyn Stream<Item = anyhow::Result<ApiAcceptedTx<Self::Confirmation>>> + Send>>,
        >,
    > {
        Some(
            self.transaction_cache
                .subscribe_starting_from_tx_number(starting_from)
                .await,
        )
    }

    async fn subscribe_blobs_from_blob_sender(
        &self,
    ) -> Option<broadcast::Receiver<BlobExecutionStatus<<Self::Da as DaService>::Spec>>> {
        Some(self.blobs_sender_channel.subscribe())
    }

    async fn update_state(
        &self,
        _update_info: StateUpdateInfo<<Self::Spec as Spec>::Storage>,
    ) -> anyhow::Result<()> {
        unimplemented!("The preferred sequencer manages its own state updates; do not call update_state() on it. If you see this, this is a bug, please report it.")
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn accept_tx(
        &self,
        baked_tx: FullyBakedTx,
    ) -> Result<AcceptedTx<Self::Confirmation>, ErrorObject> {
        if self.shutdown_receiver.has_changed().unwrap_or(true) {
            tracing::info!("The sequencer is shutting down. Cannot accept transactions");
            return Err(shut_down_error());
        }
        let original_tx_queue_id = self.tx_queue_id.load(Ordering::Acquire);

        let tx_hash = Rt::Auth::compute_tx_hash(&baked_tx).map_err(generic_accept_tx_error)?;
        tracing::debug!(%tx_hash, "Executing accept_tx");

        // Check if this transaction has a configured delay
        let runtime = Rt::default();
        let call = match Rt::Auth::decode_serialized_tx(&baked_tx) {
            Ok(call) => call,
            Err(_) => {
                return Err(ErrorObject {
                    status: StatusCode::BAD_REQUEST,
                    message: "Unable to decode transaction".to_string(),
                    details: sov_rest_utils::json_obj!({
                        "error": "Unable to decode transaction".to_string(),
                    }),
                });
            }
        };
        let call = Rt::wrap_call(call);
        let delay_ms = runtime.get_transaction_delay_ms(&call);

        if delay_ms > 0 {
            tracing::debug!(%tx_hash, delay_ms, "Delaying transaction processing");
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            tracing::debug!(%tx_hash, "Transaction delay completed, proceeding with processing");
        }

<<<<<<< HEAD
        let res = self
            .synchronized_state_updator
            .accept_tx_msg(&baked_tx, tx_hash, original_tx_queue_id, "accept_tx")
            .await;
=======
        let mut inner = self.lock_inner().await;
        let new_tx_queue_id = self.tx_queue_id.load(Ordering::Acquire);
        if new_tx_queue_id != original_tx_queue_id {
            tracing::debug!(%tx_hash, "Transaction was queued before downtime. Dropping.");
            return Err(sequencer_overloaded_503());
        }

        self.check_readiness(
            &inner,
            self.config.max_concurrent_blobs,
            self.stop_at_rollup_height,
        )
        .await
        .map_err(error_not_fully_synced)?;
>>>>>>> fix rejection in accept_tx

<<<<<<< HEAD
        match res {
            Ok(rx) => rx.await.map_err(database_error_500),
            Err(e) => match e {
                AcceptTxError::SequencerOverloaded503 => {
=======
        if let Err(e) = inner
            .try_to_create_and_start_batch_if_none_in_progress(self.stop_at_rollup_height, false)
            .await
        {
            // On all errors, we treat the sequencer as having had downtime and clear out the transaction queue.
            // Note that we'll increment the queue ID once per rejected tx. This is totally fine - we have 2**64 ids to play with
            // and atomic increments are very cheap relative to the cost of executing the tx
            self.tx_queue_id.fetch_add(1, Ordering::AcqRel);
            match e {
                BatchCreationError::NoFinalizedSlotAvailable => {
>>>>>>> Prevent sequencer from creating batches past the stop_at_height parameter
                    return Err(sequencer_overloaded_503());
                }
                AcceptTxError::NotFullySynced(details) => {
                    return Err(error_not_fully_synced(details))
                }
<<<<<<< HEAD
                AcceptTxError::BatchError {
                    batch_creation_error,
                    nb_of_concurrent_blob_submissions,
                } => match batch_creation_error {
                    BatchCreationError::NoFinalizedSlotAvailable => {
                        return Err(sequencer_overloaded_503());
                    }
                    BatchCreationError::BlobSenderBusy => {
                        return Err(error_not_fully_synced(
                            SequencerNotReadyDetails::WaitingOnBlobSender {
                                max_concurrent_blobs: self.config.max_concurrent_blobs,
                                nb_of_blobs_in_flight: nb_of_concurrent_blob_submissions,
                            },
                        ));
                    }
                    BatchCreationError::DatabaseError(e) => {
                        return Err(database_error_500(e));
                    }
                },
                AcceptTxError::TxTooBig {
                    current_batch_size,
                    max_batch_size,
                } => {
                    return Err(err_cant_fit_tx(
                        current_batch_size,
                        max_batch_size,
                        baked_tx.data.len(),
                    ))
                }
                AcceptTxError::ExecutorError(err) => {
                    return Err(RollupBlockExecutorError::into_http_error(err));
=======
                BatchCreationError::PreferredSequencerAtStopHeight {
                    height_to_stop_at,
                    current_height,
                } => {
                    return Err(error_not_fully_synced(
                        SequencerNotReadyDetails::PreferredSequencerAtStopHeight {
                            height_to_stop_at,
                            current_height,
                        },
                    ));
                }
                BatchCreationError::DatabaseError(e) => {
                    return Err(database_error_500(e));
>>>>>>> Prevent sequencer from creating batches past the stop_at_height parameter
                }

                AcceptTxError::Shutdown => {
                    return Err(shut_down_error());
                }
            },
        }
<<<<<<< HEAD
=======

        err_if_cant_fit_tx(&inner.batch_size_tracker, &baked_tx)?;

        let Inner {
            executor,
            batch_size_tracker,
            executor_events_sender,
            ..
        } = &mut *inner;

        let apply_tx_res = executor.apply_tx_to_in_progress_batch(&baked_tx).await;

        let TxReceiptWithEvents {
            receipt,
            events,
            remaining_slot_gas,
            execution_time_micros,
        } = match apply_tx_res {
            Ok(res) => {
                assert_eq!(
                    tx_hash, res.receipt.tx_hash,
                    "The executor returned a different tx hash than expected"
                );
                res
            }
            Err(err) => {
                tracing::debug!(%tx_hash, %err, "Transaction was dropped by the sequencer");
                return Err(RollupBlockExecutorError::into_http_error(err));
            }
        };
        let confirmation = Confirmation {
            events,
            receipt: receipt.receipt.into(),
        };
        batch_size_tracker.add_tx(baked_tx.data.len(), execution_time_micros);
        let rx = executor_events_sender
            .send_accept_tx(
                baked_tx,
                tx_hash,
                confirmation,
                executor
                    .checkpoint
                    .clone_with_empty_witness_dropping_temp_cache(),
            )
            .await;
        self.close_batch_if_nearly_full(&mut *inner, &remaining_slot_gas)
            .await;
        drop(inner);

        rx.await
            .map_err(database_error_500)?
            .ok_or_else(|| error_not_fully_synced(SequencerNotReadyDetails::ReplicaMode))
>>>>>>> fix rejection in accept_tx
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
}

fn shut_down_error() -> ErrorObject {
    tracing::info!("The sequencer is shutting down. Cannot accept transactions");
    ErrorObject {
        status: StatusCode::SERVICE_UNAVAILABLE,
        message: "The sequencer is shutting down".to_string(),
        details: sov_rest_utils::json_obj!({
            "error": "The sequencer is shutting down. Transactions cannot be accepted at this time".to_string(),
        }),
    }
}

#[derive(Debug)]
pub(crate) struct PreferredBatchToReplay {
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
    /// When enabled, the sequencer will skip some expensive consistency checks
    /// on the state root. This means that bugs in the implementation are less likely to be detected
    /// but may improve performance and allows the sequencer to continue operating in case of known bugs.
    #[serde(default)]
    pub disable_state_root_consistency_checks: bool,
    /// The ideal lag behind the finalized slot number.
    #[serde(default = "default_ideal_lag_behind_finalized_slot")]
    pub ideal_lag_behind_finalized_slot: u64,
    #[serde(default = "default_db_event_channel_size")]
    /// The number of events that can be buffered in the database event channel while `update_state` is running.
    /// This value needs to be increased at higher TPS to avoid blocking the sequencer.
    pub db_event_channel_size: usize,
    /// Strategy for handling recovery scenarios in the preferred sequencer.
    pub recovery_strategy: RecoveryStrategy,
    /// Target time in milliseconds to spend executing all the txs in a single batch. Batches will be closed when they exceed this value.
    pub batch_execution_time_limit_millis: u64,
    /// Time in milliseconds after which a replica will attempt to become the master if no heartbeat is observed.
    /// No effect if postgres_connection_string is not set.
    #[serde(default = "default_failover_threshold_millis")]
    pub failover_threshold_millis: u64,
}

impl Default for PreferredSequencerConfig {
    fn default() -> Self {
        Self {
            minimum_profit_per_tx: 0,
            events_channel_size: default_events_channel_size(),
            postgres_connection_string: None,
            disable_state_root_consistency_checks: false,
            ideal_lag_behind_finalized_slot: default_ideal_lag_behind_finalized_slot(),
            recovery_strategy: RecoveryStrategy::None,
            db_event_channel_size: default_db_event_channel_size(),
            batch_execution_time_limit_millis: 6_000, // 6 seconds
            failover_threshold_millis: default_failover_threshold_millis(),
        }
    }
}

/// The ideal buffer of finalized slots that the sequencer should maintain. The larger this number,
/// the longer forced transactions will take to be included but the more the sequencer is able to buffer
/// instability on the DA layer.
pub const fn default_ideal_lag_behind_finalized_slot() -> u64 {
    10
}

fn default_events_channel_size() -> usize {
    10_000
}

fn default_db_event_channel_size() -> usize {
    10_000
}

fn default_failover_threshold_millis() -> u64 {
    500
}

#[async_trait]
impl<S, Rt, Da> ProofBlobSender for PreferredSequencer<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    async fn produce_and_publish_proof_blob(&self, proof_data: Arc<[u8]>) -> anyhow::Result<()> {
        let blob_id = new_blob_id();
        self.synchronized_state_updator
            .proof_blob_msg(
                blob_id,
                proof_data.clone(),
                "produce_and_publish_proof_blob",
            )
            .await;

        Ok(())
    }
}

#[serde_with::serde_as]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TxBody(#[serde_as(as = "serde_with::base64::Base64")] Vec<u8>);

/// Transaction confirmation data of [`PreferredSequencer`].
#[derive(derivative::Derivative, serde::Serialize, serde::Deserialize)]
#[derivative(Clone(bound = ""), Debug(bound = "S: Spec, Rt: Runtime<S>"))]
#[serde(bound = "S: Spec, Rt: Runtime<S>")]
pub struct Confirmation<S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    events: Vec<RuntimeEventResponse<<Rt as RuntimeEventProcessor>::RuntimeEvent>>,
    receipt: ApiTxEffect<TxReceiptContents<S>>,
    tx_number: u64,
}

fn get_next_sequence_number_according_to_node<S, Rt>(
    latest_state_info: &StateUpdateInfo<S::Storage>,
    runtime: &mut Rt,
) -> SequenceNumber
where
    S: Spec,
    Rt: Runtime<S>,
{
    let mut checkpoint = StateCheckpoint::new(latest_state_info.storage.clone(), &runtime.kernel());
    let mut state = KernelStateAccessor::from_checkpoint(&runtime.kernel(), &mut checkpoint);

    runtime.kernel().next_sequence_number(&mut state)
}

fn is_lagging_less_than_ideal_amount(
    current_visible_slot_number: VisibleSlotNumber,
    latest_finalized_slot_number: SlotNumber,
    ideal_lag_behind_finalized_slot: u64,
) -> bool {
    latest_finalized_slot_number
        .checked_sub(current_visible_slot_number.get())
        .is_some_and(|delta| delta.get() < ideal_lag_behind_finalized_slot)
}

fn next_visible_slot_number_increase<S: Spec>(
    checkpoint: &StateCheckpoint<S>,
    info: &StateUpdateInfo<S::Storage>,
    leave_space_for_next_batch: bool,
    ideal_lag_behind_finalized_slot: u64,
) -> Result<NonZero<u8>, SequencerNotReadyDetails> {
    trace!(?checkpoint, ?info, %leave_space_for_next_batch, "Calculating next visible slot number");

    next_visible_slot_number_increase_inner(
        checkpoint.current_visible_slot_number(),
        info.latest_finalized_slot_number,
        leave_space_for_next_batch,
        ideal_lag_behind_finalized_slot,
    )
}

fn next_visible_slot_number_increase_inner(
    current_visible_slot_number: VisibleSlotNumber,
    latest_finalized_slot_number: SlotNumber,
    leave_space_for_next_batch: bool,
    ideal_lag_behind_finalized_slot: u64,
) -> Result<NonZero<u8>, SequencerNotReadyDetails> {
    let mut delta = latest_finalized_slot_number.checked_sub(current_visible_slot_number.get());

    if leave_space_for_next_batch {
        delta = delta.and_then(|x| x.checked_sub(1));
    }

    // Suppose delta = 10 and ideal_lag_behind_finalized_slot = 10. Then
    let delta_to_use = delta.map(|delta| {
        if delta.get() <= ideal_lag_behind_finalized_slot {
            delta.min(SlotNumber::ONE)
        } else {
            delta.saturating_sub(ideal_lag_behind_finalized_slot)
        }
    });

    match delta_to_use.and_then(|delta| NonZero::new(delta.get().try_into().unwrap_or(u8::MAX))) {
        Some(delta) => {
            let max_slots_to_advance = config_value!("MAX_VISIBLE_HEIGHT_INCREASE_PER_SLOT");
            let min = std::cmp::min(
                delta,
                NonZero::new(max_slots_to_advance)
                    .expect("MAX_VISIBLE_HEIGHT_INCREASE_PER_SLOT should be greater than 0"),
            );
            Ok(min)
        }
        _ => Err(SequencerNotReadyDetails::WaitingOnDa {
            finalized_slot_number: latest_finalized_slot_number,
            needed_finalized_slot_number: latest_finalized_slot_number.checked_add(1).expect(
                "Slot number overflow! This should be unreachable in the next few billion years",
            ),
        }),
    }
}

async fn prune_transactions_cache<S: Spec, Rt: Runtime<S>>(
    next_tx_number: u64,
    cache: TxResultWriter<S, Rt>,
) {
    cache.prune(next_tx_number).await;
}

/// A helper function to allow recovering an associated consant from an *instance* of a type
/// when the type itself is unknown. This is useful when a function returns `impl Trait` and we
/// want to get an associated item from that trait implementation.
fn accepts_preferred_batches<B: BlobSelector>(_blob_selector: B) -> bool {
    B::ACCEPTS_PREFERRED_BATCHES
}

fn err_cant_fit_tx(current_batch_size: usize, max_batch_size: usize, tx_len: usize) -> ErrorObject {
    ErrorObject {
        status: StatusCode::SERVICE_UNAVAILABLE,
        message: "Transaction cannot be included in the batch due to batch size limitations"
            .to_string(),
        details: sov_rest_utils::json_obj!({
            "error": "The transaction is too large.",
            "serialized_tx_size": BatchSizeTracker::serialized_tx_size(tx_len),
            "current_batch_size": current_batch_size,
            "max_batch_size": max_batch_size,
        }),
    }
}

pub(crate) async fn exit_rollup(shutdown_sender: &watch::Sender<()>) {
    // In the Kubernetes environment, logs are sometimes lost during shutdown.
    // This delay ensures logs have time to be flushed before the application exits.
    tracing::info!("Shutting down the rollup");
    if shutdown_sender.send(()).is_err() {
        tracing::error!("Failed to send shutdown signal");
    }
    sleep(Duration::from_secs(5)).await;
    tracing::info!("Calling std::process::exit(1).");
    std::process::exit(1);
}

/// An error that can occur when trying to create a new batch.
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)] // Similar to SequencerNotReadyDetails, the inner fields are self-documenting
pub enum BatchCreationError {
    /// The blob sender is applying backpressure due to difficulty landing blob on DA
    #[error("The blob sender is busy. Cannot create a new batch.")]
    BlobSenderBusy,
    /// An internal database error occurred.
    #[error("Internal database error; batch could not be created. Error: {0}")]
    DatabaseError(anyhow::Error),
    /// The sequencer was not able to start a batch because it has consumed its whole buffer of finalized slots.
    #[error("The sequencer is temporarily overloaded. Try again in a few seconds")]
    NoFinalizedSlotAvailable,
    /// The prefered sequencer has reached the stop height and is no longer creating new batches.
    #[error(
        "The sequencer is halted for a chain upgrade. Please wait for the upgrade to complete."
    )]
    PreferredSequencerAtStopHeight {
        height_to_stop_at: RollupHeight,
        current_height: RollupHeight,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_next_visible_slot_number_increase() {
        struct TestCase<'a> {
            current_visible: u64,
            latest_finalized: u64,
            leave_space: bool,
            ideal_lag: u64,
            expected: Option<u8>,
            description: &'a str,
        }

        fn run_test(case: &TestCase) {
            let result = next_visible_slot_number_increase_inner(
                VisibleSlotNumber::new_dangerous(case.current_visible),
                SlotNumber::new(case.latest_finalized),
                case.leave_space,
                case.ideal_lag,
            );

            let expected_result = case.expected.map(|val| NonZero::new(val).unwrap());

            assert_eq!(
                result.ok(),
                expected_result,
                "Test failed: {}. Input: current_visible={}, latest_finalized={}, leave_space={}, ideal_lag={}",
                case.description,
                case.current_visible,
                case.latest_finalized,
                case.leave_space,
                case.ideal_lag,
            );
        }

        let max_slots_to_advance = config_value!("MAX_VISIBLE_HEIGHT_INCREASE_PER_SLOT");

        let test_cases = &[
            TestCase {
                description: "Lag < ideal, not reserving extra space",
                current_visible: 1,
                latest_finalized: 10,
                leave_space: false,
                ideal_lag: 10,
                expected: Some(1),
            },
            TestCase {
                description: "Lag < ideal, reserving extra space",
                current_visible: 1,
                latest_finalized: 10,
                leave_space: true,
                ideal_lag: 10,
                expected: Some(1),
            },
            TestCase {
                description: "No lag, reserving extra space, should fail",
                current_visible: 1,
                latest_finalized: 1,
                leave_space: true,
                ideal_lag: 10,
                expected: None,
            },
            TestCase {
                description: "Lag of 1, reserving extra space, should fail",
                current_visible: 1,
                latest_finalized: 2,
                leave_space: true,
                ideal_lag: 10,
                expected: None,
            },
            TestCase {
                description: "Lag of 2, reserving extra space, should advance by 1",
                current_visible: 1,
                latest_finalized: 3,
                leave_space: true,
                ideal_lag: 10,
                expected: Some(1),
            },
            TestCase {
                description: "No lag, not reserving extra space, should fail",
                current_visible: 1,
                latest_finalized: 1,
                leave_space: false,
                ideal_lag: 10,
                expected: None,
            },
            TestCase {
                description: "Lag of 1, not reserving extra space, should advance by 1",
                current_visible: 1,
                latest_finalized: 2,
                leave_space: false,
                ideal_lag: 10,
                expected: Some(1),
            },
            TestCase {
                description: "Lag == ideal, reserving extra space, should advance by 1",
                current_visible: 1,
                latest_finalized: 13,
                leave_space: true,
                ideal_lag: 12,
                expected: Some(1),
            },
            TestCase {
                description: "Lag > ideal, not reserving extra space",
                current_visible: 1,
                latest_finalized: 13,
                leave_space: false,
                ideal_lag: 10,
                expected: Some(2),
            },
            TestCase {
                description: "Lag > ideal, reserving extra space",
                current_visible: 1,
                latest_finalized: 17,
                leave_space: true,
                ideal_lag: 10,
                expected: Some(5),
            },
            TestCase {
                description: "Lag > ideal, not reserving extra space",
                current_visible: 1,
                latest_finalized: 17,
                leave_space: false,
                ideal_lag: 10,
                expected: Some(6),
            },
            TestCase {
                description: "Large lag, reserving extra space, should be capped",
                current_visible: 10,
                latest_finalized: 1_000_000,
                leave_space: true,
                ideal_lag: 10,
                expected: Some(max_slots_to_advance),
            },
            TestCase {
                description: "Large lag, not reserving extra space, should be capped",
                current_visible: 10,
                latest_finalized: 1_000_000,
                leave_space: false,
                ideal_lag: 10,
                expected: Some(max_slots_to_advance),
            },
        ];

        for case in test_cases {
            run_test(case);
        }
    }
}
