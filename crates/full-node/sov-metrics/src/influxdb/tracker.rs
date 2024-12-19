//! Implementation for the [`MetricsTracker`] for Influxdb.

use std::io::Write;
use std::sync::OnceLock;

use crate::influxdb::{publisher, Metric};
use crate::{MetricsTracker, MonitoringConfig};

pub(crate) static METRICS_TRACKER: OnceLock<MetricsTracker> = OnceLock::new();

/// Alias for number of nano-seconds since unix epoch.
pub(crate) type Timestamp = u128;

/// Spawns task that published metrics in the background.
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
    fn submit(&self, measurement: SovRollupMetric) {
        // TODO: Maybe print warning if it fails?
        let _ = self.sender.try_send(Box::new(measurement));
    }

    /// Tracks all runner-related metrics
    pub fn track_runner_metrics(&self, point: RunnerMetrics) {
        let timestamp = timestamp();
        let RunnerMetrics {
            da_height: da_height_processed,
            sync_distance,
            get_block_time,
            batches_processed,
            batch_bytes_processed,
            transactions_processed,
            proofs_processed,
            proof_bytes_processed,
            process_slot_time,
            apply_slot_time,
            stf_transition_time,
            extract_blobs_time,
            extraction_proof_time,
        } = point;
        self.submit(SovRollupMetric::RunnerDa(
            timestamp,
            RunnerDaMetrics {
                da_height: da_height_processed,
                sync_distance,
                get_block_time,
            },
        ));
        self.submit(SovRollupMetric::RunnerCount(
            timestamp,
            RunnerCountMetrics {
                da_height: da_height_processed,
                batches: batches_processed,
                batch_bytes: batch_bytes_processed,
                transactions: transactions_processed,
                proofs_processed,
                proof_bytes_processed,
            },
        ));
        self.submit(SovRollupMetric::RunnerTimes(
            timestamp,
            RunnerTimeMetrics {
                da_height: da_height_processed,
                process_slot_time,
                apply_slot_time,
                stf_transition_time,
                extract_blobs_time,
                extraction_proof_time,
            },
        ));
    }

    /// Tracks processing of transaction.
    pub fn track_transaction_processing(&self, point: TransactionProcessingMetrics) {
        let timestamp = timestamp();
        self.submit(SovRollupMetric::TransactionProcessing(timestamp, point));
    }

    /// Tracks metrics related to the part of slot processing that happens in user space. Written as a single point.
    pub fn track_user_space_slot_processing(&self, point: UserSpaceSlotProcessingMetrics) {
        let timestamp = timestamp();
        self.submit(SovRollupMetric::UserSpaceSlotProcessing(timestamp, point));
    }

    /// Tracks metrics related to slot processing. Written as a single point.
    pub fn track_slot_processing(&self, point: SlotProcessingMetrics) {
        let timestamp = timestamp();
        self.submit(SovRollupMetric::SlotProcessing(timestamp, point));
    }

    /// Tracks metrics related to batch processing.
    pub fn track_batch_processing(&self, point: BatchMetrics) {
        let timestamp = timestamp();
        self.submit(SovRollupMetric::BatchProcessing(timestamp, point));
    }

    /// Tracks HTTP-related metrics.
    pub fn track_http_request(&self, point: HttpMetrics) {
        let timestamp = timestamp();
        self.submit(SovRollupMetric::Http(timestamp, point));
    }

    /// Tracks ZKVM cycles count
    pub fn track_zkvm_metric(&self, point: ZkVmCycleCount) {
        let timestamp = timestamp();
        self.submit(SovRollupMetric::ZkVm(timestamp, point));
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
    pub da_height: u64,
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

pub(crate) struct RunnerDaMetrics {
    pub da_height: u64,
    pub sync_distance: i64,
    pub get_block_time: std::time::Duration,
}

pub(crate) struct RunnerCountMetrics {
    pub da_height: u64,
    pub batches: u64,
    pub batch_bytes: u64,
    pub transactions: u64,
    pub proofs_processed: u64,
    pub proof_bytes_processed: u64,
}

pub(crate) struct RunnerTimeMetrics {
    pub da_height: u64,
    pub process_slot_time: std::time::Duration,
    pub apply_slot_time: std::time::Duration,
    pub stf_transition_time: std::time::Duration,
    pub extract_blobs_time: std::time::Duration,
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
    /// Time it took from slot initialization till blobs have been selected.
    /// Includes kernel and state initialization + chain_state logic.
    pub blobs_selection_time: std::time::Duration,

    /// Time it took to materialize slot changes and finalize slot hooks.
    pub slot_finalization_time: std::time::Duration,

    /// Height of DA layer when this slot has been applied.
    pub da_height: u64,

    /// Visible rollup height at given slot.
    pub rollup_height: u64,

    /// [`sov_rollup_interface::stf::ExecutionContext`]
    pub execution_context: sov_rollup_interface::stf::ExecutionContext,
}

/// Metrics related to processing of a single slot.
pub struct UserSpaceSlotProcessingMetrics {
    /// Time it took for begin slot hooks.
    /// Includes KernelSlotHooks and normal SlotHooks.
    pub begin_slot_hooks_time: std::time::Duration,

    /// Time it took to process all blobs: Batches, Proofs and Forced registration
    pub blobs_processing_time: std::time::Duration,

    /// Time it took for end slot hooks.
    pub end_slot_hooks_time: std::time::Duration,

    /// Visible rollup height at given slot.
    pub rollup_height: u64,

    /// [`sov_rollup_interface::stf::ExecutionContext`]
    pub execution_context: sov_rollup_interface::stf::ExecutionContext,
}

impl Metric for RunnerDaMetrics {
    fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
        write!(
            buffer,
            "sov_rollup_runner_da da_height={},sync_distance={},get_block_time_ms={}",
            self.da_height,
            self.sync_distance,
            self.get_block_time.as_millis(),
        )
    }
}

impl Metric for RunnerCountMetrics {
    fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
        write!(
            buffer,
            "sov_rollup_runner_counts da_height={},batches_c={},transactions_c={},proofs_c={},batch_bytes={},proof_bytes={}",
            self.da_height,
            self.batches,
            self.transactions,
            self.proofs_processed,
            self.batch_bytes,
            self.proof_bytes_processed,
        )
    }
}

impl Metric for RunnerTimeMetrics {
    fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
        write!(
            buffer,
            "sov_rollup_runner_times_us da_height={},process_slot={},apply_slot={},stf_transition={},extract_blobs={},blob_extraction_proof={}",
            self.da_height,
            self.process_slot_time.as_micros(),
            self.apply_slot_time.as_micros(),
            self.stf_transition_time.as_micros(),
            self.extract_blobs_time.as_micros(),
            self.extraction_proof_time.as_micros(),
        )
    }
}

impl Metric for TransactionProcessingMetrics {
    fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
        write!(buffer, "sov_rollup_transaction_execution_us,status={:?},context={:?},call_message={},sequencer={} value={},rollup_height={}",
               // tags
               self.tx_effect,
               self.execution_context,
               self.call_message,
               self.sequencer_address,
               //fields
               self.execution_time.as_micros(),
               self.rollup_height,
        )
    }
}

impl Metric for SlotProcessingMetrics {
    fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
        write!(
            buffer,
            "sov_rollup_slot_execution_time_us,context={:?} blobs_selection={},finalization={},rollup_height={},da_height={}",
            // Tags
            self.execution_context,
            // Fields
            self.blobs_selection_time.as_micros(),
            self.slot_finalization_time.as_micros(),
            self.rollup_height,
            self.da_height,
        )
    }
}

impl Metric for UserSpaceSlotProcessingMetrics {
    fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
        write!(
            buffer,
            "sov_rollup_slot_execution_time_us,context={:?} begin_hooks={},blobs_processing={},end_hooks={},rollup_height={}",
            // Tags
            self.execution_context,
            // Fields
            self.begin_slot_hooks_time.as_micros(),
            self.blobs_processing_time.as_micros(),
            self.end_slot_hooks_time.as_micros(),
            self.rollup_height,
        )
    }
}

/// Simplified version of a batch outcome.
#[derive(Debug)]
pub enum BatchOutcome {
    #[allow(missing_docs)]
    Executed,
    #[allow(missing_docs)]
    Ignored,
}

/// Metrics for batch with transactions.
pub struct BatchMetrics {
    #[allow(missing_docs)]
    pub processing_time: std::time::Duration,
    /// Number of transactions have been processed in batch.
    pub transactions_count: usize,
    #[allow(missing_docs)]
    pub outcome: BatchOutcome,
}

impl Metric for BatchMetrics {
    fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
        write!(
            buffer,
            "sov_rollup_batch_processing,outcome={:?} processing_time_us={},transactions={}",
            self.outcome,
            self.processing_time.as_micros(),
            self.transactions_count,
        )
    }
}

/// Metrics for an HTTP subsystem.
/// Can be applied to REST API or JSON RPC.
pub struct HttpMetrics {
    /// HTTP method.
    pub request_method: http::Method,
    /// URI being requested.
    pub request_uri: http::Uri,
    /// Status code of the response.
    pub response_status: http::StatusCode,
    /// Approximate size of the response body.
    pub response_body_size: u64,
    /// Time it took for the inner handler to finish processing.
    /// Does not include request reading and response writing.
    pub handler_processing_time: std::time::Duration,
}

impl Metric for HttpMetrics {
    fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
        write!(
            buffer,
            "sov_rollup_http_handlers,req_method={},resp_status={},path={} processing_time_us={},response_body_bytes={}",
            // Tags
            self.request_method,
            self.response_status.as_u16(),
            self.request_uri.path(),
            // Fields
            self.handler_processing_time.as_micros(),
            self.response_body_size,
        )
    }
}

/// Representation of cycle count for particular call
pub struct ZkVmCycleCount {
    /// Name of the caller site, usually a function or method
    pub name: String,
    /// Number of ZKVM cycles have been spent on this call.
    pub cycles_count: u64,
}

impl Metric for ZkVmCycleCount {
    fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
        write!(
            buffer,
            "sov_rollup_zkvm,name={} cycles_count={}",
            self.name, self.cycles_count
        )
    }
}

enum SovRollupMetric {
    RunnerDa(Timestamp, RunnerDaMetrics),
    RunnerCount(Timestamp, RunnerCountMetrics),
    RunnerTimes(Timestamp, RunnerTimeMetrics),
    SlotProcessing(Timestamp, SlotProcessingMetrics),
    UserSpaceSlotProcessing(Timestamp, UserSpaceSlotProcessingMetrics),
    BatchProcessing(Timestamp, BatchMetrics),
    TransactionProcessing(Timestamp, TransactionProcessingMetrics),
    Http(Timestamp, HttpMetrics),
    ZkVm(Timestamp, ZkVmCycleCount),
}

impl Metric for SovRollupMetric {
    fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
        let timestamp = match self {
            SovRollupMetric::RunnerCount(t, m) => {
                m.serialize_for_telegraf(buffer)?;
                t
            }
            SovRollupMetric::RunnerDa(t, m) => {
                m.serialize_for_telegraf(buffer)?;
                t
            }
            SovRollupMetric::RunnerTimes(t, m) => {
                m.serialize_for_telegraf(buffer)?;
                t
            }
            SovRollupMetric::SlotProcessing(t, m) => {
                m.serialize_for_telegraf(buffer)?;
                t
            }
            SovRollupMetric::BatchProcessing(t, m) => {
                m.serialize_for_telegraf(buffer)?;
                t
            }
            SovRollupMetric::TransactionProcessing(t, m) => {
                m.serialize_for_telegraf(buffer)?;
                t
            }
            SovRollupMetric::Http(t, m) => {
                m.serialize_for_telegraf(buffer)?;
                t
            }
            SovRollupMetric::UserSpaceSlotProcessing(t, m) => {
                m.serialize_for_telegraf(buffer)?;
                t
            }
            SovRollupMetric::ZkVm(t, m) => {
                m.serialize_for_telegraf(buffer)?;
                t
            }
        };
        write!(buffer, " {}", timestamp)
    }
}
