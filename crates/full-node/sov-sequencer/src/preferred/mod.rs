//! See [`PreferredSequencer`].

mod async_batch;
mod batch_size_tracker;
mod block_executor;
mod db;
mod preferred_blob_sender;
mod state_root_compute;

use std::boxed::Box;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::num::NonZero;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::http::StatusCode;
use batch_size_tracker::BatchSizeTracker;
use db::postgres::PostgresBackend;
use db::rocksdb::RocksDbBackend;
use db::{PreferredSequencerDb, PreferredSequencerDbBackend, PreferredSequencerReadBlob};
use preferred_blob_sender::PreferredBlobSender;
use schemars::JsonSchema;
use serde_with::serde_as;
use sov_blob_sender::{new_blob_id, BlobSender};
use sov_blob_storage::{PreferredBatchData, SequenceNumber};
use sov_db::ledger_db::LedgerDb;
use sov_modules_api::capabilities::{BlobSelector, TransactionAuthenticator};
use sov_modules_api::macros::config_value;
use sov_modules_api::rest::utils::ErrorObject;
use sov_modules_api::rest::{ApiState, StateUpdateReceiver};
use sov_modules_api::{
    ApiTxEffect, FullyBakedTx, RejectReason, Runtime, RuntimeEventProcessor, RuntimeEventResponse,
    Spec, StateCheckpoint, StateUpdateInfo, VersionReader, VisibleSlotNumber, *,
};
use sov_rest_utils::errors::database_error_500;
use sov_rollup_interface::common::SlotNumber;
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::node::ledger_api::{EventIdentifier, LedgerStateProvider};
use sov_rollup_interface::node::DaSyncState;
use sov_rollup_interface::TxHash;
use sov_state::{NativeStorage, Storage};
use state_root_compute::StateRootBackgroundTaskState;
use tokio::sync::mpsc::{self, Sender};
use tokio::sync::{broadcast, watch, Mutex, MutexGuard};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, trace, warn, Instrument};

use crate::common::{
    error_not_fully_synced, generic_accept_tx_error, loop_call_update_state,
    loop_send_tx_notifications, AcceptedTx, Sequencer, TxStatusBlobSenderHooks, WithCachedTxHashes,
};
use crate::metrics::{track_in_progress_batch_size, PreferredSequencerUpdateStateMetrics};
use crate::preferred::block_executor::{EventCache, RollupBlockExecutor, RollupBlockExecutorError};
use crate::{
    ProofBlobSender, SequencerConfig, SequencerEvent, SequencerNotReadyDetails, TxStatus,
    TxStatusManager,
};

type VisibleSlotNumberIncrease = NonZero<u8>;

/// A inner sequencer struct containing state that requires synchronized access.
struct Inner<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    db: PreferredSequencerDb<S, Rt>,
    latest_info: StateUpdateInfo<S::Storage>,
    checkpoint_sender: watch::Sender<StateCheckpoint<S>>,
    blob_sender: PreferredBlobSender<Da>,
    config: SequencerConfig<S::Da, S::Address, PreferredSequencerConfig>,
    shutdown_receiver: watch::Receiver<()>,

    executor: RollupBlockExecutor<S, Rt>,
    batch_size_tracker: BatchSizeTracker,
    is_ready: Result<(), SequencerNotReadyDetails>,
}

impl<S, Rt, Da> Inner<S, Rt, Da>
where
    S: Spec,
    Rt: Runtime<S>,
    Da: DaService<Spec = S::Da>,
{
    fn blob_sender_busy(&self) -> Option<usize> {
        let num_current_in_flight = self.blob_sender.nb_of_concurrent_blob_submissions();

        if num_current_in_flight > self.config.max_concurrent_blobs {
            Some(num_current_in_flight)
        } else {
            None
        }
    }

    /// Syncs [`ApiState`]s with the latest [`StateCheckpoint`].
    #[tracing::instrument(skip_all, level = "trace")]
    async fn update_api_state(&self, checkpoint: StateCheckpoint<S>) {
        self.checkpoint_sender.send(
            checkpoint
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
    ) -> Result<(), ErrorObject> {
        if self.executor.has_in_progress_batch() {
            return Ok(());
        }

        let Ok(visible_increase) = next_visible_slot_number_increase(
            &self.executor.checkpoint,
            &self.latest_info,
            leave_space_for_next_batch,
        ) else {
            return Ok(());
        };

        debug!(visible_increase, "No in-progress batch, starting a new one");

        let node_state_root = self.node_root_hash().map_err(database_error_500)?;
        let visible_slot_number_after_increase = self
            .executor
            .checkpoint
            .current_visible_slot_number()
            .advance(visible_increase.get().into());

        // If the database operation fails here it's okay because we still
        // haven't touched the background task nor modified `self`, so
        // everything will be left in a valid state.
        self.db
            .start_batch(visible_slot_number_after_increase, visible_increase)
            .await
            .map_err(database_error_500)?;

        let min_profit_per_tx = self.config.sequencer_kind_config.minimum_profit_per_tx;
        self.executor
            .start_rollup_block(
                visible_slot_number_after_increase,
                visible_increase,
                &node_state_root,
                min_profit_per_tx,
            )
            .await;

        Ok(())
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn trigger_batch_production(&mut self) -> anyhow::Result<()> {
        if !self.config.automatic_batch_production {
            warn!("Skipping batch production due to settings");
            return Ok(());
        }

        // Check if we have enough slots to create a new batch immediately after
        // this one. If we don't, let's not assemble a batch.
        //
        // TODO(@neysofu): this check is currently necessary but likely can be folded into
        // `try_to_create_and_start_batch_if_none_in_progress`... somehow. As of
        // right now, it's a hair too bug-prone.
        if next_visible_slot_number_increase(&self.executor.checkpoint, &self.latest_info, true)
            .is_err()
        {
            return Ok(());
        }

        if self.blob_sender_busy().is_some() {
            return Ok(());
        }

        // If there's no in-progress batch, we open a new one and immediately
        // close it. This will result in an empty batch, which has the sole
        // purpose of increasing the visible slot number.
        self.try_to_create_and_start_batch_if_none_in_progress(true)
            .await
            .map_err(|_| anyhow::anyhow!("Unable to start a new batch; this is likely a database issue or a bug, please report it"))?;

        // We were unable to open a new batch (likely due to a lack of finalized
        // slots), so we're done.
        if !self.executor.has_in_progress_batch() {
            return Ok(());
        }

        // If the node is shutting down, we may not be able to terminate the batch. In that case, just return early.
        if self.shutdown_receiver.has_changed().unwrap_or(true) {
            return Ok(());
        }

        // Terminate the batch.
        let batch = self.db.terminate_batch().await?;
        self.executor.end_rollup_block().await;
        self.batch_size_tracker = BatchSizeTracker::new(self.config.max_batch_size_bytes);

        self.update_api_state(
            self.executor
                .checkpoint
                .clone_with_empty_witness_dropping_temp_cache(),
        )
        .await;

        // Publish the batch.
        let tx_hashes: Arc<[TxHash]> = batch.tx_hashes.clone().into();
        self.blob_sender
            .hooks()
            .add_txs(batch.blob_id, tx_hashes.clone())
            .await;
        self.blob_sender.publish_batch(batch).await?;

        // Update the metrics.
        track_in_progress_batch_size(
            self.db
                .in_progress_batch_opt()
                .map(|b| b.txs.len() as u64)
                .unwrap_or(0),
        );

        Ok(())
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
    shutdown_notifier: Sender<()>,
    state_root_compute_task: StateRootBackgroundTaskState<S>,
    shutdown_receiver: watch::Receiver<()>,
    ledger_db: LedgerDb,
    cached_events: EventCache<RuntimeEventResponse<Rt::RuntimeEvent>>,
}

struct SoftConfirmationsReplayReceipt {
    inner_lock_start_time: std::time::Instant,
    metrics: PreferredSequencerUpdateStateMetrics,
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
    pub async fn create(
        da: Da,
        state_update_receiver: StateUpdateReceiver<S::Storage>,
        da_sync_state: Arc<DaSyncState>,
        storage_path: &Path,
        config: &SequencerConfig<S::Da, S::Address, PreferredSequencerConfig>,
        ledger_db: LedgerDb,
        shutdown_sender: watch::Sender<()>,
    ) -> anyhow::Result<(Arc<Self>, Vec<JoinHandle<()>>)> {
        let shutdown_receiver = shutdown_sender.subscribe();
        let latest_state_update = state_update_receiver.borrow().clone();
        debug!(
            ?latest_state_update,
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
        let completed_blobs = db_backend.read_completed_blobs().await?;

        let blob_sender = {
            let (inner, blob_sender_handle) = BlobSender::new(
                da,
                ledger_db.clone(),
                storage_path,
                TxStatusBlobSenderHooks::new(tx_status_manager.clone()),
                shutdown_sender,
            )
            .await?;

            handles.push(blob_sender_handle);

            let mut blob_sender = PreferredBlobSender::from(inner);

            // It's possible that sov-blob-sender's DB might miss some blob data at
            // node startup due to:
            //  1. Disk failure (the sequencer can use Postgres so it's durable).
            //  2. DB corruption.
            //  3. Node crash at an inconvenient time.
            // Let's restore all missing blob data to make sure they land on the DA.
            blob_sender.publish_blobs(completed_blobs).await?;
            blob_sender
        };

        let (state_root_compute_handle, state_root_compute_task) =
            StateRootBackgroundTaskState::create(
                shutdown_notifier.clone(),
                shutdown_receiver.clone(),
                !config
                    .sequencer_kind_config
                    .disable_state_root_consistency_checks,
            );
        handles.push(state_root_compute_handle);

        let cached_events = Arc::new(tokio::sync::RwLock::new(BTreeMap::new()));

        let inner = Inner {
            db: PreferredSequencerDb::<S, Rt>::new(db_backend).await?,
            latest_info: latest_state_update.clone(),
            checkpoint_sender,
            config: config.clone(),
            shutdown_receiver: shutdown_receiver.clone(),
            blob_sender,
            executor: RollupBlockExecutor::new(
                latest_state_update.clone(),
                Some(events_sender.clone()),
                config.clone(),
                shutdown_notifier.clone(),
                state_root_compute_task.request_sender.clone(),
                shutdown_receiver.clone(),
                cached_events.clone(),
            ),
            batch_size_tracker: BatchSizeTracker::new(config.max_batch_size_bytes),
            is_ready: Ok(()),
        };

        let seq = Arc::new(PreferredSequencer {
            inner: inner.into(),
            tx_status_manager: tx_status_manager.clone(),
            events_sender,
            da_sync_state,
            api_state,
            _runtime: PhantomData,
            shutdown_notifier,
            config: config.clone(),
            state_root_compute_task,
            shutdown_receiver: shutdown_receiver.clone(),
            ledger_db: ledger_db.clone(),
            cached_events,
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

    #[tracing::instrument(skip_all, level = "debug")]
    async fn lock_inner(&self) -> MutexGuard<Inner<S, Rt, Da>> {
        self.inner.lock().await
    }

    async fn replay_soft_confirmations_on_top_of_node_state(
        &self,
        info: &StateUpdateInfo<S::Storage>,
        batches_to_replay: Vec<PreferredBatchToReplay>,
    ) -> anyhow::Result<(MutexGuard<Inner<S, Rt, Da>>, SoftConfirmationsReplayReceipt)> {
        let batches_count = batches_to_replay.len() as u64;
        let transactions_count = batches_to_replay
            .iter()
            .map(|b| b.batch.inner.data.len() as u64)
            .sum::<u64>();

        if tracing::enabled!(tracing::Level::TRACE) {
            let batch_details_to_log = batches_to_replay
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

        // Now that we're not locking on the sequencer state anymore, we can replay all the batches.
        let mut executor = RollupBlockExecutor::<_, Rt>::new(
            info.clone(),
            None, // We don't re-send events when replaying batches in the background.
            self.config.clone(),
            self.shutdown_notifier.clone(),
            self.state_root_compute_task.request_sender.clone(),
            self.shutdown_receiver.clone(),
            self.cached_events.clone(),
        );

        let node_state_root = tracing::trace_span!("root_hash")
            .in_scope(|| info.storage.get_root_hash(info.slot_number))?;
        let last_batch = batches_to_replay.last();
        let last_replayed_batch_in_progress = last_batch.map(|batch| batch.is_in_progress);
        let latest_batch_txs_len = last_batch.map(|batch| batch.batch.tx_hashes.len());

        async {
            for batch in batches_to_replay {
                executor.replay_batch(&batch, &node_state_root).await?;
                if self.shutdown_receiver.has_changed().unwrap_or(true) {
                    return Ok(());
                }
            }
            Ok::<(), anyhow::Error>(())
        }
        .instrument(tracing::debug_span!("process_batches"))
        .await?;

        // We stop accepting new txns in accept_tx for a short time while we catch up
        let mut inner = self.lock_inner().await;
        let inner_lock_start_time = std::time::Instant::now();

        let current_in_progress_batch = inner.db.in_progress_batch_opt().cloned();

        let in_progress_batch_exists = current_in_progress_batch.is_some();

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
                let batch = PreferredBatchToReplay {
                    is_in_progress: true,
                    visible_slot_number_after_increase: in_progress_batch
                        .visible_slot_number_after_increase,
                    batch: in_progress_batch.into(),
                };
                let node_root = inner.node_root_hash()?;

                if executor.replay_batch(&batch, &node_root).await? {
                    inner.db.pop_tx_from_in_progress_batch().await?;
                }
            }
            _ => trace!("No new transaction or batch state while updating node state"),
        }

        let metrics = PreferredSequencerUpdateStateMetrics {
            duration: Duration::ZERO,
            lock_duration: Duration::ZERO,
            batches_count,
            transactions_count,
            in_progress_batch: in_progress_batch_exists,
        };

        trace!("Node state update complete, swapping new state into sequencer");
        inner.executor.replace_state(executor).await;

        Ok((
            inner,
            SoftConfirmationsReplayReceipt {
                inner_lock_start_time,
                metrics,
            },
        ))
    }

    async fn lock_inner_once_theres_enough_finalized_slots(
        &self,
    ) -> Result<MutexGuard<Inner<S, Rt, Da>>, SequencerNotReadyDetails> {
        let start = std::time::Instant::now();

        while start.elapsed() < Duration::from_secs(10) {
            let inner = self.lock_inner().await;

            // We cannot accept transactions until the latest finalized slot number
            // is AT LEAST 1. Meaning, as long as we're stuck at genesis, we can't
            // accept any transactions.
            //
            // If we're still stuck at genesis, we'll wait a bit and try again.
            //
            // > Why not just respond with an error, and refuse transactions until
            // > the node is ready?
            // Because plenty of scripts, examples, and tests rely on the
            // assumption that the sequencer is immediately ready to accept
            // transactions. So, we cheat a little bit and wait it out, so API
            // consumers never have to deal with such erros (that is, unless we
            // timeout, which I'd expect will never happen if the node is
            // healthy and indexing slots as intended).
            if inner.latest_info.latest_finalized_slot_number == SlotNumber::GENESIS {
                warn!("Accepting a transaction, but the node must progress beyond genesis first");

                // Drop the lock and give `update_state` a chance to run while we sleep.
                drop(inner);
                tokio::time::sleep(Duration::from_secs(1)).await;
            } else {
                return Ok(inner);
            }
        }

        tracing::error!("Timed out while waiting for the node to progress beyond genesis. The sequencer can't accept transactions until that happens");

        Err(SequencerNotReadyDetails::WaitingOnDa {
            finalized_slot_number: SlotNumber::GENESIS,
            needed_finalized_slot_number: SlotNumber::new(1),
        })
    }

    async fn lock_inner_if_ready(
        &self,
    ) -> Result<MutexGuard<Inner<S, Rt, Da>>, SequencerNotReadyDetails> {
        let inner = self.lock_inner_once_theres_enough_finalized_slots().await?;

        if let Some(nb_of_blobs_in_flight) = inner.blob_sender_busy() {
            return Err(SequencerNotReadyDetails::WaitingOnBlobSender {
                max_concurrent_blobs: self.config.max_concurrent_blobs,
                nb_of_blobs_in_flight,
            });
        }

        inner.is_ready.as_ref().map_err(|details| details.clone())?;

        Ok(inner)
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
    type Spec = S;
    type Rt = Rt;
    type Da = Da;

    async fn list_events(
        &self,
        event_nums: &[u64],
    ) -> Result<
        Vec<RuntimeEventResponse<<Self::Rt as RuntimeEventProcessor>::RuntimeEvent>>,
        anyhow::Error,
    > {
        trace!(events_len = event_nums.len(), "listing events");

        let (mut events, missing_ids) = {
            let mut events = vec![];
            let mut cache_misses = vec![];
            let cached_events = self.cached_events.read().await;

            for event_num in event_nums {
                let event = cached_events.get(event_num).cloned();
                if let Some((event, _)) = event {
                    events.push(event);
                } else {
                    cache_misses.push(EventIdentifier::Number(*event_num));
                }
            }
            (events, cache_misses)
        };

        let ledger_events = self
            .ledger_db
            .get_events::<RuntimeEventResponse<Rt::RuntimeEvent>>(&missing_ids)
            .await?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();

        debug!(
            cache_hits = events.len(),
            cache_misses = missing_ids.len(),
            "retrieved sequencer events"
        );

        events.extend(ledger_events);

        trace!(result_len = events.len(), "retrieved events");

        Ok(events)
    }

    async fn is_ready(&self) -> Result<(), SequencerNotReadyDetails> {
        // We don't actually care about the `inner`, we just want to reuse the
        // same logic.
        self.lock_inner_if_ready().await.map(|_| ())
    }

    fn api_state(&self) -> ApiState<Self::Spec> {
        self.api_state.clone()
    }

    #[tracing::instrument(skip_all, level = "debug")]
    async fn update_state(&self, info: StateUpdateInfo<S::Storage>) -> anyhow::Result<()> {
        let timer_start = std::time::Instant::now();
        let mut rt = Rt::default();

        prune_events_cache(info.latest_finalized_slot_number, &self.cached_events).await;

        // We gotta briefly lock to access the database, but release the lock ASAP.
        let (batches_to_replay, next_sequence_number) = {
            let mut inner = self.lock_inner().await;

            (
                batches_to_replay(&mut inner.db, &info).await?,
                inner.db.next_sequence_number(),
            )
        };

        let next_sequence_number_according_to_node =
            next_sequence_number_according_to_node(&info, &mut rt);
        let distance = self.da_sync_state.status().distance();

        let condition_nodes_sequence_number_is_fresher =
            next_sequence_number_according_to_node > next_sequence_number;

        // Once we're this close to `deferred_slots_count`, we risk crossing the
        // `deferred_slots_count` threshold before the next call to
        // `update_state`. That's no good.
        let condition_too_close_to_deferred_slots_count_for_comfort = info.slot_number.delta(
            StateCheckpoint::new(info.storage.clone(), &rt.kernel())
                .current_visible_slot_number()
                .as_true(),
        )
            > sov_blob_storage::config_deferred_slots_count().saturating_mul(90) / 100;

        // Resuming operations while the node is
        // lagging can cause issues e.g. during failover or after sequencer DB
        // deletion due to in-flight blobs that are not yet processed.
        let condition_node_is_lagging = distance > self.config.max_allowed_node_distance_behind;

        // Are there ANY soft confirmations to replay at all?
        let condition_are_there_any_batches_to_replay = !batches_to_replay.is_empty();

        tracing::debug!(
            condition_nodes_sequence_number_is_fresher,
            condition_too_close_to_deferred_slots_count_for_comfort,
            condition_node_is_lagging,
            condition_are_there_any_batches_to_replay,
            "Choosing the state update code path"
        );

        match (
            condition_nodes_sequence_number_is_fresher,
            condition_too_close_to_deferred_slots_count_for_comfort,
            condition_node_is_lagging,
            condition_are_there_any_batches_to_replay,
        ) {
            // Something has gone terribly wrong, and I don't see a way for us
            // to meaningfully recover without nuking the sequencer DB.
            (true, _, _, true) => {
                panic!("The node has a higher sequence number than the sequencer, but the sequencer has some batches that it must replay (i.e. we're not just re-indexing a chain starting from an empty sequencer DB). This is an unusual scenario. It could mean you're running a competing preferred sequencer (which is not allowed!), or your sequencer DB data is corrupted... or it's just a bug. Please report it. You might attempt to recover by deleting the entire sequencer DB.")
            }
            // We found a preferred batch of which we have no memory very close
            // to the chain tip.
            (true, _, false, false) => {
                warn!("The node has a higher sequence number than the sequencer, but we're very close to the chain tip (i.e. we're not simply syncing). This could mean there is another preferred sequencer running, or you very recently restarted the node and there's still some in-flight blobs. Sleeping for a while...");

                let mut inner = self.lock_inner().await;
                inner
                    .db
                    .overwrite_next_sequence_number(next_sequence_number_according_to_node);

                // We very intentionally keep the lock for all this time, and
                // prevent any transactions from being accepted.
                //
                // This works well, but incoming accept_tx requests will likely
                // timeout instead of receiving an error. TODO: improve devex (#2937).
                tokio::time::sleep(Duration::from_secs(10)).await;

                inner.latest_info = info.clone();
                // We update the API state, so users can query node state as it syncs.
                let checkpoint = StateCheckpoint::new(info.storage.clone(), &rt.kernel());
                inner.update_api_state(checkpoint).await;
            }
            // We are either dangerously close to hitting the
            // `deferred_slots_count` threshold or we've hit it already. Our
            // soft-confirmations might easily get invalidated.
            (false, true, false, any_batches_to_replay) => {
                error!(
                    "Recovery mode due to deferred slots count threshold, {:?}",
                    any_batches_to_replay
                );
                std::process::exit(1);
            }
            // The node is lagging behind the chain tip. Pause the sequencer (if
            // it wasn't already paused), and wait for the node to catch up.
            (must_overwrite, _, true, any_batches_to_replay) => {
                assert!(!(must_overwrite && any_batches_to_replay), "Sanity check failed - this condition was already handled above. If you see this, please report it.");
                if any_batches_to_replay {
                    warn!(?distance, "The sequencer must pause because the node is lagging behind. This might lead to a brief downtime for users. Cause is unknown.");
                } else {
                    // We find ourselvers in a very standard syncing scenario.
                    info!(?distance, "Pausing the sequencer while the node syncs");
                }

                let mut inner = self.lock_inner().await;
                inner.is_ready = Err(SequencerNotReadyDetails::Syncing {
                    target_da_height: self.da_sync_state.target_da_height.load(Ordering::Relaxed),
                    synced_da_height: self.da_sync_state.synced_da_height.load(Ordering::Relaxed),
                });

                if must_overwrite {
                    inner
                        .db
                        .overwrite_next_sequence_number(next_sequence_number_according_to_node);
                }

                inner.latest_info = info.clone();
                // We update the API state, so users can query node state as it syncs.
                let checkpoint = StateCheckpoint::new(info.storage.clone(), &rt.kernel());
                inner.update_api_state(checkpoint).await;
            }
            // This is by far the most common scenario, i.e. a nominal
            // `update_state` call during sequencer execution with no unusual
            // conditions.
            (false, false, false, _) => {
                let (mut inner, receipt) = self
                    .replay_soft_confirmations_on_top_of_node_state(&info, batches_to_replay)
                    .await?;

                inner.is_ready = Ok(());

                let SoftConfirmationsReplayReceipt {
                    inner_lock_start_time,
                    mut metrics,
                } = receipt;

                inner.latest_info = info.clone();
                let checkpoint = inner
                    .executor
                    .checkpoint
                    .clone_with_empty_witness_dropping_temp_cache();
                inner.update_api_state(checkpoint).await;

                sov_metrics::track_metrics(|t| {
                    metrics.duration = timer_start.elapsed();
                    metrics.lock_duration = inner_lock_start_time.elapsed();

                    t.submit(metrics);
                });

                if !self.shutdown_receiver.has_changed().unwrap_or(true) {
                    inner.trigger_batch_production().await?;
                }
            }
        }

        trace!("Sequencer state update completed successfully");
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
        if self.shutdown_receiver.has_changed().unwrap_or(true) {
            return Err(ErrorObject {
                status: StatusCode::SERVICE_UNAVAILABLE,
                title: "The sequencer is shutting down".to_string(),
                details: sov_rest_utils::json_obj!({
                    "message": "The sequencer is shutting down. Transactions cannot be accepted at this time".to_string(),
                }),
            });
        }

        let tx_hash = Rt::Auth::compute_tx_hash(&baked_tx).map_err(generic_accept_tx_error)?;
        tracing::debug!(%tx_hash, "Executing accept_tx");

        let mut inner = self
            .lock_inner_if_ready()
            .await
            .map_err(error_not_fully_synced)?;

        inner
            .try_to_create_and_start_batch_if_none_in_progress(false)
            .await?;

        if !inner.executor.has_in_progress_batch() {
            panic!(
                "No batch in progress, and no batch could be started. Please report this bug. {:?} {:?}",
                &inner.executor.checkpoint, inner.latest_info
            );
        }

        err_if_cant_fit_tx(&inner.batch_size_tracker, &baked_tx)?;

        let Inner {
            executor,
            db,
            batch_size_tracker,
            ..
        } = &mut *inner;

        let apply_tx_res = executor.apply_tx_to_in_progress_batch(&baked_tx).await;

        let (receipt, events) = match apply_tx_res {
            Ok(res) => res,
            Err(err) => {
                tracing::debug!(%tx_hash, %err, "Transaction was dropped by the sequencer");
                return Err(RollupBlockExecutorError::into_http_error(err));
            }
        };

        db.insert_tx(baked_tx.clone(), tx_hash)
            .await
            .map_err(database_error_500)?;

        batch_size_tracker.add_tx(baked_tx.data.len());
        tracing::debug!(%tx_hash, "Transaction was accepted by the sequencer");

        track_in_progress_batch_size(
            db.in_progress_batch_opt()
                .map(|b| b.txs.len() as u64)
                .unwrap_or(0),
        );

        inner
            .update_api_state(
                inner
                    .executor
                    .checkpoint
                    .clone_with_empty_witness_dropping_temp_cache(),
            )
            .await;

        Ok(AcceptedTx {
            tx: baked_tx,
            tx_hash,
            confirmation: Confirmation {
                events,
                receipt: receipt.receipt.into(),
            },
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
}

struct PreferredBatchToReplay {
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
}

impl Default for PreferredSequencerConfig {
    fn default() -> Self {
        Self {
            minimum_profit_per_tx: 0,
            events_channel_size: default_events_channel_size(),
            postgres_connection_string: None,
            disable_state_root_consistency_checks: false,
        }
    }
}

fn default_events_channel_size() -> usize {
    10_000
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
        let mut inner = self.inner.lock().await;

        let sequence_number = inner
            .db
            .insert_proof_blob(blob_id, proof_data.clone())
            .await?;

        inner
            .blob_sender
            .publish_proof(proof_data, sequence_number, blob_id)
            .await?;

        Ok(())
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

#[tracing::instrument(skip_all, level = "trace")]
async fn batches_to_replay<S, Rt>(
    db: &mut PreferredSequencerDb<S, Rt>,
    info: &StateUpdateInfo<S::Storage>,
) -> anyhow::Result<Vec<PreferredBatchToReplay>>
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
            PreferredSequencerReadBlob::Batch(batch) => Some(PreferredBatchToReplay {
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

    if let Some(batch) = db.in_progress_batch_opt().cloned() {
        batches.push(PreferredBatchToReplay {
            is_in_progress: true,
            visible_slot_number_after_increase: batch.visible_slot_number_after_increase,
            batch: batch.into(),
        });
    }

    Ok(batches)
}

fn next_sequence_number_according_to_node<S, Rt>(
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
            finalized_slot_number: info.latest_finalized_slot_number,
            needed_finalized_slot_number: info.latest_finalized_slot_number.checked_add(1).expect(
                "Slot number overflow! This should be unreachable in the next few billion years",
            ),
        }),
    }
}

async fn prune_events_cache<E>(finialized_slot: SlotNumber, cache: &EventCache<E>) {
    let mut writer = cache.write().await;
    writer.retain(|_, (_, slot_num)| *slot_num > finialized_slot);
}

/// A helper function to allow recovering an associated consant from an *instance* of a type
/// when the type itself is unknown. This is useful when a function returns `impl Trait` and we
/// want to get an associated item from that trait implementation.
fn accepts_preferred_batches<B: BlobSelector>(_blob_selector: B) -> bool {
    B::ACCEPTS_PREFERRED_BATCHES
}

fn err_if_cant_fit_tx(tracker: &BatchSizeTracker, tx: &FullyBakedTx) -> Result<(), ErrorObject> {
    if !tracker.can_fit_tx(tx.data.len()) {
        return Err(ErrorObject {
            status: StatusCode::SERVICE_UNAVAILABLE,
            title: "Transaction cannot be included in the batch due to batch size limitations"
                .to_string(),
            details: sov_rest_utils::json_obj!({
                "message": "The transaction is too large.",
                "serialized_tx_size": BatchSizeTracker::serialized_tx_size(tx.data.len()),
                "current_batch_size": tracker.current_batch_size,
                "max_batch_size": tracker.max_batch_size,
            }),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_prune_events_cache() {
        let cached_events = Arc::new(tokio::sync::RwLock::new(BTreeMap::new()));
        {
            let mut writer = cached_events.write().await;
            writer.insert(1, ((), SlotNumber::new(1)));
            writer.insert(2, ((), SlotNumber::new(2)));
            writer.insert(3, ((), SlotNumber::new(3)));
            writer.insert(4, ((), SlotNumber::new(4)));
            writer.insert(5, ((), SlotNumber::new(5)));
        }
        prune_events_cache(SlotNumber::new(3), &cached_events).await;

        let reader = cached_events.read().await;
        assert_eq!(reader.len(), 2);
        assert_eq!(reader.get(&4), Some(&((), SlotNumber::new(4))));
        assert_eq!(reader.get(&5), Some(&((), SlotNumber::new(5))));
    }
}
