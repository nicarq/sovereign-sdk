//! Implementation for the [`MetricsTracker`] for Influxdb.

use std::io::Write;
#[cfg(feature = "native")]
use std::sync::OnceLock;

#[cfg(feature = "native")]
use crate::influxdb::publisher;
use crate::MetricsTracker;
#[cfg(feature = "native")]
use crate::MonitoringConfig;

#[cfg(feature = "native")]
pub(crate) static METRICS_TRACKER: OnceLock<MetricsTracker> = OnceLock::new();

/// Spawns task that published metrics in the background.
#[cfg(feature = "native")]
pub fn init_metrics_tracker(
    shutdown_receiver: tokio::sync::watch::Receiver<()>,
    config: &MonitoringConfig,
) -> tokio::task::JoinHandle<()> {
    // Commented code will allow to properly reinitialization in the same process
    // https://github.com/rust-lang/rust/issues/121641
    //  Currently, the publishing task is started on every init, but metrics are dropped
    // because OnceLock holds sender to the task that has been started first time.
    //
    // let was_initialized_before = METRICS_TRACKER.get().is_some();
    let (sender, receiver) = tokio::sync::mpsc::channel(config.get_max_pending_metrics() as usize);
    let config = config.clone();
    let handle = tokio::spawn(async move {
        publisher::metrics_publisher_task(shutdown_receiver, receiver, &config).await;
    });
    OnceLock::get_or_init(&METRICS_TRACKER, || {
        tracing::trace!("Metrics tracker initialized");
        MetricsTracker { sender }
    });
    // https://github.com/rust-lang/rust/issues/121641
    // let mut tracker = OnceLock::get_mut_or_init(&METRICS_TRACKER, || {
    //     tracing::trace!("Metrics tracker initialized");
    //     MetricsTracker {
    //         sender: sender.clone(),
    //     }
    // });
    // if was_initialized_before {
    //     tracker.sender = sender;
    // }

    handle
}

impl MetricsTracker {
    const PROCESSED_DA_HEIGHT: [u8; 30] = *b"sov_rollup_processed_da_height";
    const SYNC_DISTANCE: [u8; 24] = *b"sov_rollup_sync_distance";
    const GET_BLOCK_TIME: [u8; 23] = *b"sov_rollup_get_block_ms";

    const BATCHES_PROCESSED: [u8; 28] = *b"sov_rollup_batches_processed";
    const BATCH_BYTES_PROCESSED: [u8; 32] = *b"sov_rollup_batch_bytes_processed";
    const PROOFS_PROCESSED: [u8; 27] = *b"sov_rollup_proofs_processed";
    const PROOF_BYTES_PROCESSED: [u8; 32] = *b"sov_rollup_proof_bytes_processed";
    const TRANSACTIONS_PROCESSED: [u8; 33] = *b"sov_rollup_transactions_processed";
    const TRANSACTION_EXECUTION_TIME: [u8; 35] = *b"sov_rollup_transaction_execution_us";
    const SLOT_EXECUTION_TIME: [u8; 33] = *b"sov_rollup_slot_execution_time_us";
    const PROCESS_SLOT_TIME: [u8; 26] = *b"sov_rollup_process_slot_ms";
    const APPLY_SLOT_TIME: [u8; 24] = *b"sov_rollup_apply_slot_ms";
    const STF_TRANSITION_TIME: [u8; 28] = *b"sov_rollup_stf_transition_ms";
    const EXTRACT_RELEVANT_BLOBS_TIME: [u8; 27] = *b"sov_rollup_extract_blobs_us";
    const EXTRACTION_PROOF_TIME: [u8; 35] = *b"sov_rollup_blob_extraction_proof_us";

    fn submit(&self, measurement: Vec<u8>) {
        // TODO: Maybe print warning if it fails?
        let _ = self.sender.try_send(measurement);
    }

    fn submit_with_value_only_with_timestamp(
        &self,
        metric_name: &[u8],
        value: impl std::fmt::Display,
        timestamp: u128,
    ) {
        let mut measurement = metric_name.to_vec();
        write!(measurement, " value={} {}", value, timestamp).unwrap();
        self.submit(measurement);
    }

    /// Tracks all runner-related metrics
    pub fn track_runner_metrics(&self, point: RunnerMetrics) {
        let timestamp = timestamp();
        self.submit_with_value_only_with_timestamp(
            &Self::SYNC_DISTANCE,
            point.sync_distance,
            timestamp,
        );
        self.submit_with_value_only_with_timestamp(
            &Self::PROCESSED_DA_HEIGHT,
            point.da_height_processed,
            timestamp,
        );
        self.submit_with_value_only_with_timestamp(
            &Self::GET_BLOCK_TIME,
            point.get_block_time.as_millis(),
            timestamp,
        );
        self.submit_with_value_only_with_timestamp(
            &Self::BATCHES_PROCESSED,
            point.batches_processed,
            timestamp,
        );
        self.submit_with_value_only_with_timestamp(
            &Self::BATCH_BYTES_PROCESSED,
            point.batch_bytes_processed,
            timestamp,
        );
        self.submit_with_value_only_with_timestamp(
            &Self::PROOFS_PROCESSED,
            point.proofs_processed,
            timestamp,
        );
        self.submit_with_value_only_with_timestamp(
            &Self::PROOF_BYTES_PROCESSED,
            point.proof_bytes_processed,
            timestamp,
        );
        self.submit_with_value_only_with_timestamp(
            &Self::TRANSACTIONS_PROCESSED,
            point.transactions_processed,
            timestamp,
        );
        self.submit_with_value_only_with_timestamp(
            &Self::PROCESS_SLOT_TIME,
            point.process_slot_time.as_millis(),
            timestamp,
        );
        self.submit_with_value_only_with_timestamp(
            &Self::APPLY_SLOT_TIME,
            point.apply_slot_time.as_millis(),
            timestamp,
        );
        self.submit_with_value_only_with_timestamp(
            &Self::STF_TRANSITION_TIME,
            point.stf_transition_time.as_millis(),
            timestamp,
        );
        self.submit_with_value_only_with_timestamp(
            &Self::EXTRACT_RELEVANT_BLOBS_TIME,
            point.extract_blobs_time.as_micros(),
            timestamp,
        );
        self.submit_with_value_only_with_timestamp(
            &Self::EXTRACTION_PROOF_TIME,
            point.extraction_proof_time.as_micros(),
            timestamp,
        );
    }

    /// Tracks processing of transaction.
    pub fn track_transaction_processing(&self, point: TransactionProcessingMetrics) {
        let timestamp = timestamp();
        let mut measurement = Self::TRANSACTION_EXECUTION_TIME.to_vec();
        write!(
            measurement,
            ",status={:?},context={:?},call_message={},sequencer={} value={},rollup_height={} {}",
            point.tx_effect,
            point.execution_context,
            point.call_message,
            point.sequencer_address,
            point.execution_time.as_micros(),
            point.rollup_height,
            timestamp
        )
        .unwrap();
        self.submit(measurement);
    }

    /// Tracks metrics related to slot processing. Written as a single point.
    pub fn track_slot_processing(&self, point: SlotProcessingMetrics) {
        let timestamp = timestamp();
        let mut measurement = Self::SLOT_EXECUTION_TIME.to_vec();
        write!(
            measurement,
            ",context={:?} blobs_selection={},begin_hooks={},blobs_processing={},end_hooks={},finalization={},rollup_height={},da_height={} {}",
            // Tags
            point.execution_context,
            // Fields
            point.blobs_selection_time.as_micros(),
            point.begin_slot_hooks_time.as_micros(),
            point.blobs_processing_time.as_micros(),
            point.end_slot_hooks_time.as_micros(),
            point.slot_finalization_time.as_micros(),
            point.rollup_height,
            point.da_height,
            // Timestamp
            timestamp
        )
        .unwrap();
        self.submit(measurement);
    }
}

pub(crate) fn timestamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Metrics related to the main loop of STF runner.
pub struct RunnerMetrics {
    /// DA height processed in this iteration.
    pub da_height_processed: u64,
    /// Distance between processed DA height and DA head.
    pub sync_distance: i64,
    /// Time it took to fetch given block from DA layer.
    pub get_block_time: std::time::Duration,
    /// Number of batches with transactions processed in this iteration.
    pub batches_processed: u64,
    /// Total size of blobs with transactions being processed.
    pub batch_bytes_processed: u64,
    /// Total number of transactions being processed
    pub transactions_processed: u64,
    /// Total number of proofs being processed in this iteration.
    pub proofs_processed: u64,
    /// Total size of proofs.
    pub proof_bytes_processed: u64,
    /// Full time it took to process slot.
    /// Includes all operations required together with pre/post-processing.
    /// process_slot_time == (get_block_time + extract_blobs_time + extraction_proof_time + stf_transition_time)
    pub process_slot_time: std::time::Duration,
    /// Time it took to execute only STF transition without post-processing or committing to the DB.
    pub apply_slot_time: std::time::Duration,
    /// Time it took to execute the STF transition, post-process, and commit all results to the DB,
    /// but without fetching data from DA and extracting it.
    pub stf_transition_time: std::time::Duration,
    /// Time it took to extract relevant blobs from the whole DA block.
    pub extract_blobs_time: std::time::Duration,
    /// Time it took to build proof that the relevant blobs were extracted correctly.
    pub extraction_proof_time: std::time::Duration,
}

/// Simplified version of [`sov_rollup_interface::stf::TxEffect`]
#[derive(Debug)]
pub enum TransactionEffect {
    /// The transaction was skipped.
    Skipped,
    /// The transaction was reverted during execution.
    Reverted,
    /// The transaction was processed successfully.
    Successful,
}

impl<T: sov_rollup_interface::stf::TxReceiptContents> From<&sov_rollup_interface::stf::TxEffect<T>>
    for TransactionEffect
{
    fn from(value: &sov_rollup_interface::stf::TxEffect<T>) -> Self {
        match value {
            sov_rollup_interface::stf::TxEffect::Skipped(_) => TransactionEffect::Skipped,
            sov_rollup_interface::stf::TxEffect::Reverted(_) => TransactionEffect::Reverted,
            sov_rollup_interface::stf::TxEffect::Successful(_) => TransactionEffect::Successful,
        }
    }
}

/// Collection of metrics related to transaction processing.
pub struct TransactionProcessingMetrics {
    /// Time it took a transaction to be executed
    pub execution_time: std::time::Duration,
    /// The effect of the transaction,
    /// simplified version of [`sov_rollup_interface::stf::TxEffect`]
    pub tx_effect: TransactionEffect,
    /// [`sov_rollup_interface::stf::ExecutionContext`]
    pub execution_context: sov_rollup_interface::stf::ExecutionContext,
    /// Height at which transaction is being executed
    pub rollup_height: u64,
    /// Human-readable address of sequencer.
    pub sequencer_address: String,
    /// Call message
    pub call_message: String,
}

/// Metrics related to processing of a single slot.
pub struct SlotProcessingMetrics {
    /// Time it took from slot initialization, till blobs have been selected.
    /// Includes kernel and state initialization + chain_state logic.
    pub blobs_selection_time: std::time::Duration,

    /// Time it took for begin slot hooks.
    /// Includes KernelSlotHooks and normal SlotHooks.
    pub begin_slot_hooks_time: std::time::Duration,

    /// Time it took to process all blobs: Batches, Proofs and Forced registration
    pub blobs_processing_time: std::time::Duration,

    /// Time it took for end slot hooks.
    pub end_slot_hooks_time: std::time::Duration,

    /// Time it took to materialize slot changes and finalize slot hooks.
    pub slot_finalization_time: std::time::Duration,

    /// Height of DA layer when this slot has been applied.
    pub da_height: u64,
    /// Visible rollup height at given slot.
    pub rollup_height: u64,
    /// [`sov_rollup_interface::stf::ExecutionContext`]
    pub execution_context: sov_rollup_interface::stf::ExecutionContext,
}
