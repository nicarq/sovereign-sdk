use std::num::NonZero;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::anyhow;
use sov_blob_sender::BlobInternalId;
use sov_blob_storage::SequenceNumber;
use sov_modules_api::capabilities::RollupHeight;
use sov_modules_api::{
    FullyBakedTx, GasArray, GasSpec, Runtime, Spec, StateCheckpoint, StateUpdateInfo,
    VersionReader, VisibleSlotNumber,
};
use sov_state::{NativeStorage, Storage};
use tokio::sync::{oneshot, watch, MutexGuard};
use tracing::{debug, error, info, warn};

use super::batch_size_tracker::BatchSizeTracker;
use crate::metrics::{
    track_sequence_number, PreferredSequencerLockMetrics, PreferredSequencerLockMetricsBatch,
};
use crate::preferred::block_executor::{
    AcceptedTxWithBudgetInfo, RollupBlockExecutor, RollupBlockExecutorError,
};
use crate::preferred::db::latest_finalized_sequence_number;
use crate::preferred::executor_events::ExecutorEventsSender;
use crate::preferred::{
    current_visible_slot_number_according_to_node, exit_rollup,
    get_next_sequence_number_according_to_node, is_lagging_less_than_ideal_amount,
    next_visible_slot_number_increase, AcceptedTx, BatchCreationError, Confirmation,
    PreferredBatchToReplay, PreferredSequencerConfig,
    PreferredSequencerFetchBatchesToReplayMetrics, RecoveryStrategy, RollupBlockExecutorConfig,
};
use crate::{SequencerConfig, SequencerNotReadyDetails, SlotNumber, TxHash};

/// These two constants are used to calculate the comfortable batch size limit.
/// Currently, this is 99% of the hard limit. After the comfortable limit is reached,
/// the sequencer will close and publish the current batch.
const COMFORTABLE_SIZE_LIMIT_MULTIPLIER: u64 = 99;
const COMFORTABLE_SIZE_LIMIT_DIVISOR: u64 = 100;

/// These two constants are used to calculate the comfortable gas limit.
/// Currently, this is 95% of the initial gas limit. After the comfortable limit is reached,
/// the sequencer will close and publish the current batch.
const COMFORTABLE_GAS_LIMIT_MULTIPLIER: u64 = 19;
const COMFORTABLE_GAS_LIMIT_DIVISOR: u64 = 20;

const LOCK_METRICS_BATCH_SIZE: usize = 32;

/// A inner sequencer struct containing state that requires synchronized access.
/// This struct accepts/rejects transactions, then hands them to the side effects task
/// to be persisted.
pub(crate) struct Inner<S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    pub(crate) latest_info: StateUpdateInfo<S::Storage>,
    pub(crate) batch_execution_time_limit_micros: u64,
    pub(crate) config: SequencerConfig<S::Address, PreferredSequencerConfig>,
    pub(crate) shutdown_receiver: watch::Receiver<()>,
    pub(crate) shutdown_sender: watch::Sender<()>,
    pub(crate) executor: RollupBlockExecutor<S, Rt>,
    pub(crate) batch_size_tracker: BatchSizeTracker,
    pub(crate) is_ready: Result<(), SequencerNotReadyDetails>,
    pub(crate) in_flight_blobs: Arc<AtomicUsize>,
    pub(crate) executor_events_sender: ExecutorEventsSender<S, Rt>,
    pub(crate) sequence_number_of_next_blob: SequenceNumber,
    /// A boolean that indicates whether the sequencer has finished its startup phase.
    /// We need this rather than relying on `SequencerNotReadyDetails::Startup` because that state
    /// can be overwritten when the node is resyncing.
    pub(crate) has_finished_startup: bool,
    pub(crate) metrics: Vec<PreferredSequencerLockMetrics>,
    // Shared between sequencer and Inner.
    pub(crate) tx_queue_id: Arc<AtomicU64>,
    pub(crate) stop_at_rollup_height: Option<RollupHeight>,
}

pub(crate) struct InnerGuard<'a, S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    inner: MutexGuard<'a, Inner<S, Rt>>,
    reason: &'static str,
    start_time: std::time::Instant,
}

impl<'a, S, Rt> InnerGuard<'a, S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    /// Create a new inner guard.
    pub fn new(inner: MutexGuard<'a, Inner<S, Rt>>, reason: &'static str) -> Self {
        Self {
            inner,
            reason,
            start_time: std::time::Instant::now(),
        }
    }
}

impl<S, Rt> Deref for InnerGuard<'_, S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    type Target = Inner<S, Rt>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<S, Rt> std::ops::DerefMut for InnerGuard<'_, S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<S, Rt> Drop for InnerGuard<'_, S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    fn drop(&mut self) {
        self.inner.metrics.push(PreferredSequencerLockMetrics {
            duration: self.start_time.elapsed(),
            lock_reason: self.reason,
        });
        if self.inner.metrics.len() >= LOCK_METRICS_BATCH_SIZE {
            sov_metrics::track_metrics(|t| {
                t.submit(PreferredSequencerLockMetricsBatch {
                    metrics: std::mem::replace(
                        &mut self.inner.metrics,
                        Vec::with_capacity(LOCK_METRICS_BATCH_SIZE),
                    ),
                });
            });
        }
    }
}

impl<S, Rt> Inner<S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    pub(crate) fn nb_of_concurrent_blob_submissions(&self) -> usize {
        self.in_flight_blobs.load(Ordering::Acquire)
    }

    pub async fn publish_proof_blob(&mut self, blob_id: BlobInternalId, data: Arc<[u8]>) {
        let sequence_number = self.get_and_inc_next_sequence_number();
        self.executor_events_sender
            .publish_proof_blob(blob_id, data, sequence_number)
            .await;
    }

    pub(crate) async fn overwrite_next_sequence_number_for_recovery(
        &mut self,
        sequence_number: SequenceNumber,
    ) {
        info!(%sequence_number, "Overwriting next sequence number");
        self.sequence_number_of_next_blob = sequence_number;
        track_sequence_number(self.sequence_number_of_next_blob);
    }

    pub(crate) fn blob_sender_busy(&self) -> Option<usize> {
        let num_current_in_flight = self.nb_of_concurrent_blob_submissions();

        if num_current_in_flight > self.config.max_concurrent_blobs {
            Some(num_current_in_flight)
        } else {
            None
        }
    }

    pub(crate) fn node_root_hash(&self) -> anyhow::Result<<S::Storage as Storage>::Root> {
        self.latest_info
            .storage
            .get_root_hash(self.latest_info.slot_number)
    }

    pub(crate) fn current_height(&self) -> RollupHeight {
        self.executor.checkpoint.rollup_height_to_access()
    }

    /// Create a new batch, if possible. Errors here are expected, because it's not always possible to create a new batch due to transient DA issues.
    /// We can only create a new batch if we have a finalized slot available to use as our `visible_slot_number_after_increase`.
    #[tracing::instrument(skip_all, level = "trace")]
    pub(crate) async fn try_to_create_and_start_batch_if_none_in_progress(
        &mut self,
        leave_space_for_next_batch: bool,
    ) -> Result<(), BatchCreationError> {
        if self.executor.has_in_progress_batch() {
            return Ok(());
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
            .start_batch(
                visible_slot_number_after_increase,
                visible_increase,
                sequence_number,
                self.executor
                    .checkpoint
                    .clone_with_empty_witness_dropping_temp_cache(),
            )
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
            .start_batch(
                visible_slot_number_after_increase,
                visible_slots_to_advance,
                sequence_number,
                self.executor
                    .checkpoint
                    .clone_with_empty_witness_dropping_temp_cache(),
            )
            .await;

        Ok(())
    }

    #[tracing::instrument(skip_all, level = "trace")]
    pub(crate) async fn trigger_batch_production_if_convenient(&mut self) {
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
            .try_to_create_and_start_batch_if_none_in_progress(true)
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

    pub(crate) fn next_sequence_number(&self) -> SequenceNumber {
        self.sequence_number_of_next_blob
    }

    pub(crate) fn current_sequence_number(&self) -> SequenceNumber {
        self.sequence_number_of_next_blob.checked_sub(1).expect("Sequence number underflow. Cannot get sequence number if no batch has ever been active. This is a bug, please report")
    }

    pub(crate) fn get_and_inc_next_sequence_number(&mut self) -> SequenceNumber {
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
    pub(crate) async fn close_current_batch(&mut self) {
        // Terminate the batch.
        self.executor.end_rollup_block().await;
        self.batch_size_tracker = BatchSizeTracker::new(self.config.max_batch_size_bytes);
        let checkpoint = self
            .executor
            .checkpoint
            .clone_with_empty_witness_dropping_temp_cache();
        self.executor_events_sender.close_batch(checkpoint).await;
    }

    pub(crate) async fn prune_sequencer_db(&mut self) {
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
                    self.executor_events_sender.prune(num).await;
                }
            }
            None => {
                // Nothing to prune because there's no sequence number history.
            }
        }
    }

    pub(crate) async fn force_overwrite_state(
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
            .force_update_api_state(checkpoint)
            .await;
    }

    pub(crate) async fn check_readiness(
        &self,
        max_concurrent_blobs: usize,
        height_to_stop_at: Option<RollupHeight>,
    ) -> Result<(), SequencerNotReadyDetails> {
        // We cannot accept transactions until the latest finalized slot number
        // is AT LEAST 1. Meaning, as long as we're stuck at genesis, we can't
        // accept any transactions.
        if self.latest_info.latest_finalized_slot_number == SlotNumber::GENESIS {
            tracing::error!("Timed out while waiting for the node to progress beyond genesis. The sequencer can't accept transactions until that happens");
            return Err(SequencerNotReadyDetails::WaitingOnDa {
                finalized_slot_number: SlotNumber::GENESIS,
                needed_finalized_slot_number: SlotNumber::new(1),
            });
        }

        if let Some(nb_of_blobs_in_flight) = self.blob_sender_busy() {
            return Err(SequencerNotReadyDetails::WaitingOnBlobSender {
                max_concurrent_blobs,
                nb_of_blobs_in_flight,
            });
        }

        if let Some(height_to_stop_at) = height_to_stop_at {
            let current_height = self.current_height();
            if current_height > height_to_stop_at {
                return Err(SequencerNotReadyDetails::PreferredSequencerAtStopHeight {
                    current_height,
                    height_to_stop_at,
                });
            }
        }

        self.is_ready.as_ref().map_err(|details| details.clone())?;
        Ok(())
    }

    pub(crate) async fn trigger_recovery(
        &mut self,
        recovery_strategy: RecoveryStrategy,
        info: &StateUpdateInfo<S::Storage>,
        rollup_exec_config: RollupBlockExecutorConfig<S, Rt>,
        is_replica: bool,
    ) {
        if is_replica {
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

        self.is_ready = Err(SequencerNotReadyDetails::PreferredSequencerRecovering);
        let next_sequence_number_according_to_node =
            get_next_sequence_number_according_to_node(info, &mut Rt::default());
        self.executor_events_sender
            .trigger_recovery(next_sequence_number_according_to_node, recovery_strategy)
            .await;

        let executor_from_info = RollupBlockExecutor::<_, Rt>::new(info, rollup_exec_config);

        self.force_overwrite_state(info.clone(), executor_from_info)
            .await;

        info!(?info, current_visible_slot_number = %current_visible_slot_number_according_to_node::<S,Rt>(info), "Beginning sequencer recovery");
    }

    #[tracing::instrument(skip_all, level = "trace")]
    pub(crate) fn completed_batches_to_replay(
        &self,
        sequence_number: SequenceNumber,
        include_in_progress_batch: bool,
    ) -> (
        Vec<PreferredBatchToReplay>,
        PreferredSequencerFetchBatchesToReplayMetrics,
    )
    where
        S: Spec,
        Rt: Runtime<S>,
    {
        let start = std::time::Instant::now();
        let result = self
            .executor_events_sender
            .fetch_completed_blobs_by_sequence(sequence_number, include_in_progress_batch);
        let duration = start.elapsed();
        let metrics = PreferredSequencerFetchBatchesToReplayMetrics {
            duration,
            num_batches: result.len() as u64,
            num_transactions: result.iter().map(|b| b.batch.inner.data.len()).sum(),
        };
        (result, metrics)
    }

    /// Closes the current batch if it is nearly full (by gas limit) or has reached the target batch execution time.
    pub(crate) async fn close_batch_if_nearly_full(
        &mut self,
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
            self.close_current_batch().await;
        }

        let current_batch_execution_time_micros =
            self.batch_size_tracker.batch_execution_time_micros;

        if current_batch_execution_time_micros > self.batch_execution_time_limit_micros {
            tracing::debug!(%self.batch_execution_time_limit_micros, %current_batch_execution_time_micros, "Closing and publishing current batch because we've reached the batch execution time cap");
            self.close_current_batch().await;
        } else {
            tracing::trace!(%self.batch_execution_time_limit_micros, %current_batch_execution_time_micros, "Batch execution time is within comfortable range, not closing batch");
        }

        let comfortable_size_limit = (self.batch_size_tracker.max_batch_size as u64)
            .checked_div(COMFORTABLE_SIZE_LIMIT_DIVISOR)
            .and_then(|x| x.checked_mul(COMFORTABLE_SIZE_LIMIT_MULTIPLIER))
            .unwrap_or_else(|| {
                panic!(
                    "Cannot overflow after dividing by {COMFORTABLE_SIZE_LIMIT_DIVISOR} and multiplying by {COMFORTABLE_SIZE_LIMIT_MULTIPLIER}",
                )
            });
        if (self.batch_size_tracker.current_batch_size as u64) > comfortable_size_limit {
            tracing::debug!(%comfortable_size_limit, current_batch_size = %self.batch_size_tracker.current_batch_size, "Closing and publishing current batch because we're close to the size limit");
            self.close_current_batch().await;
        } else {
            tracing::trace!(%comfortable_size_limit, current_batch_size = %self.batch_size_tracker.current_batch_size, "Batch size is within comfortable range, not closing batch");
        }
    }
}

#[derive(Debug)]
pub(crate) enum AcceptTxError<S: Spec> {
    SequencerOverloaded503,
    NotFullySynced(SequencerNotReadyDetails),
    BatchError {
        batch_creation_error: BatchCreationError,
        nb_of_concurrent_blob_submissions: usize,
    },
    TxTooBig {
        current_batch_size: usize,
        max_batch_size: usize,
    },
    ExecutorError(RollupBlockExecutorError<S>),
    Shutdown,
}

impl<S, Rt> Inner<S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    pub(crate) async fn process_accept_tx(
        &mut self,
        baked_tx: &FullyBakedTx,
        tx_hash: TxHash,
        original_tx_queue_id: u64,
    ) -> Result<oneshot::Receiver<AcceptedTx<Confirmation<S, Rt>>>, AcceptTxError<S>> {
        // If the sequencer had to give out 503s at any point during the time we were waiting for the lock, we need to return a 503 - otherwise
        // we've effectively jumped the line
        let new_tx_queue_id = self.tx_queue_id.load(Ordering::Acquire);
        if new_tx_queue_id != original_tx_queue_id {
            tracing::debug!(%tx_hash, "Transaction was queued before downtime. Dropping.");
            return Err(AcceptTxError::SequencerOverloaded503);
        }

        self.check_readiness(self.config.max_concurrent_blobs, self.stop_at_rollup_height)
            .await
            .map_err(AcceptTxError::NotFullySynced)?;

        if let Err(batch_creation_error) = self
            .try_to_create_and_start_batch_if_none_in_progress(false)
            .await
        {
            // On all errors, we treat the sequencer as having had downtime and clear out the transaction queue.
            // Note that we'll increment the queue ID once per rejected tx. This is totally fine - we have 2**64 ids to play with
            // and atomic increments are very cheap relative to the cost of executing the tx
            self.tx_queue_id.fetch_add(1, Ordering::AcqRel);

            return Err(AcceptTxError::BatchError {
                batch_creation_error,
                nb_of_concurrent_blob_submissions: self.nb_of_concurrent_blob_submissions(),
            });
        };

        if self.shutdown_receiver.has_changed().unwrap_or(true) {
            tracing::info!("The sequencer is shutting down. Cannot accept transactions");
            return Err(AcceptTxError::Shutdown);
        }

        if !self.executor.has_in_progress_batch() {
            panic!(
                "No batch in progress, and no batch could be started. Please report this bug. {:?} {:?}",
                &self.executor.checkpoint, self.latest_info
            );
        }

        let sequence_number = self.current_sequence_number();
        let Inner {
            executor,
            batch_size_tracker,
            executor_events_sender,
            ..
        } = &mut *self;

        if !batch_size_tracker.can_fit_tx_bytes(baked_tx.data.len()) {
            return Err(AcceptTxError::TxTooBig {
                current_batch_size: batch_size_tracker.current_batch_size,
                max_batch_size: batch_size_tracker.max_batch_size,
            });
        }

        let apply_tx_res = executor.apply_tx_to_in_progress_batch(baked_tx).await;

        let (
            AcceptedTxWithBudgetInfo {
                accepted_tx,
                remaining_slot_gas,
                execution_time_micros,
            },
            tx_changes,
        ) = match apply_tx_res {
            Ok(res) => {
                assert_eq!(
                    tx_hash, res.0.accepted_tx.tx_hash,
                    "The executor returned a different tx hash than expected"
                );
                res
            }
            Err(err) => {
                tracing::debug!(%tx_hash, %err, "Transaction was dropped by the sequencer");
                return Err(AcceptTxError::ExecutorError(err));
            }
        };

        batch_size_tracker.add_tx(baked_tx.data.len(), execution_time_micros);
        let rx = executor_events_sender
            .send_accept_tx(accepted_tx, tx_changes, sequence_number)
            .await;

        self.close_batch_if_nearly_full(&remaining_slot_gas).await;

        Ok(rx)
    }
}
