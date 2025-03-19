//! Implementation for the [`MetricsTracker`] for Influxdb.

use std::collections::HashMap;
use std::fmt::Debug;
use std::io::Write;
use std::sync::{LazyLock, OnceLock, RwLock};

use sov_rollup_interface::common::VisibleSlotNumber;

#[cfg(feature = "gas-constant-estimation")]
use crate::influxdb::gas_constant_estimation::GasConstantMetric;
use crate::influxdb::{publisher, safe_telegraf_string, Metric};
use crate::{MetricsTracker, MonitoringConfig};

pub(crate) static METRICS_TRACKER: OnceLock<MetricsTracker> = OnceLock::new();

/// Global metadata that is added as measurement key/value pairs of the metrics.
// TODO: Investigate if we actually need this.
pub static METRICS_METADATA: LazyLock<RwLock<HashMap<String, String>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

#[cfg(feature = "gas-constant-estimation")]
pub mod gas_constant_estimation {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::io::Write;

    use crate::influxdb::Metric;

    thread_local! {
        /// A map of gas constants and their associated weight.
        pub static GAS_CONSTANTS: RefCell<HashMap<String, i64>> = RefCell::new(HashMap::new());
    }

    #[derive(Debug)]
    pub struct GasConstantMetric {
        pub constant: String,
        pub num_invocations: u64,
    }

    impl Metric for GasConstantMetric {
        fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
            write!(
                buffer,
                "sov_rollup_gas_constant,constant={} num_invocations={}",
                self.constant, self.num_invocations
            )?;
            Ok(())
        }
    }
}

/// Alias for number of nano-seconds since unix epoch.
pub(crate) type Timestamp = u128;

/// Spawns task that published metrics in the background.
pub fn init_metrics_tracker(config: &MonitoringConfig) {
    match METRICS_TRACKER.get() {
        None => {
            let (sender, receiver) =
                tokio::sync::mpsc::channel(config.get_max_pending_metrics() as usize);
            let config = config.clone();
            let _handle = tokio::spawn(async move {
                publisher::metrics_publisher_task(receiver, &config).await;
            });
            tracing::trace!("Metrics tracker initialized");
            OnceLock::set(&METRICS_TRACKER, MetricsTracker { sender })
                .expect("Metrics tracker failed to set metrics");
        }
        Some(_) => {}
    }
}

impl MetricsTracker {
    pub(crate) fn submit(&self, measurement: SovRollupMetric) {
        tracing::trace!(?measurement, "Submitting a measurement");
        if let Err(e) = self.sender.try_send(Box::new(measurement)) {
            tracing::trace!(error = ?e, "Dropped measurement");
        };
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

    /// Tracks ZKVM cycles count.
    pub fn track_zkvm_metric(&self, point: ZkVmExecutionChunk) {
        let timestamp = timestamp();
        self.submit(SovRollupMetric::ZkVm(timestamp, point));
    }

    /// Wall clock time for proving.
    pub fn track_zk_proving_time(&self, point: ZkProvingTime) {
        let timestamp = timestamp();
        self.submit(SovRollupMetric::ZkProving(timestamp, point));
    }

    /// Track custom metric. Timestamp is added at this moment.
    pub fn track_custom<M: Metric + 'static>(&self, measurement: M) {
        let timestamp = timestamp();
        self.submit(SovRollupMetric::Custom(timestamp, Box::new(measurement)));
    }
}

/// Returns the current timestamp in nanoseconds since the UNIX epoch.
pub fn timestamp() -> u128 {
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

#[derive(Debug)]
pub(crate) struct RunnerDaMetrics {
    pub da_height: u64,
    pub sync_distance: i64,
    pub get_block_time: std::time::Duration,
}

#[derive(Debug)]
pub(crate) struct RunnerCountMetrics {
    pub da_height: u64,
    pub batches: u64,
    pub batch_bytes: u64,
    pub transactions: u64,
    pub proofs_processed: u64,
    pub proof_bytes_processed: u64,
}

#[derive(Debug)]
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
#[derive(Debug)]
pub struct TransactionProcessingMetrics {
    /// Time it took a transaction to be executed
    pub execution_time: std::time::Duration,
    /// The effect of the transaction,
    /// simplified version of [`sov_rollup_interface::stf::TxEffect`]
    pub tx_effect: TransactionEffect,
    /// [`sov_rollup_interface::stf::ExecutionContext`]
    pub execution_context: sov_rollup_interface::stf::ExecutionContext,
    /// Height at which transaction is being executed
    pub visible_slot_number: VisibleSlotNumber,
    /// Human-readable address of sequencer.
    pub sequencer_address: String,
    /// Call message
    pub call_message: String,
}

/// Metrics related to processing of a single slot.
#[derive(Debug)]
pub struct SlotProcessingMetrics {
    /// Time it took from slot initialization till blobs have been selected.
    /// Includes kernel and state initialization + chain_state logic.
    pub blobs_selection_time: std::time::Duration,

    /// Time it took to materialize slot changes and finalize slot hooks.
    pub slot_finalization_time: std::time::Duration,

    /// Height of DA layer when this slot has been applied.
    pub da_height: u64,

    /// Visible slot number at given slot.
    pub visible_slot_number: VisibleSlotNumber,

    /// [`sov_rollup_interface::stf::ExecutionContext`]
    pub execution_context: sov_rollup_interface::stf::ExecutionContext,

    /// Gas used during slot processing, expressed in gas units
    pub gas_used: Vec<u64>,
}

/// Metrics related to processing of a single slot.
#[derive(Debug)]
pub struct UserSpaceSlotProcessingMetrics {
    /// Time taken by begin_rollup_block hook.
    pub begin_block_hook_time: std::time::Duration,

    /// Time it took to process all blobs: Batches, Proofs and Forced registration
    pub blobs_processing_time: std::time::Duration,

    /// Time taken by end_rollup_block hook.
    pub end_block_hook_time: std::time::Duration,

    /// The visible slot number associated with these metrics.
    pub visible_slot_number: VisibleSlotNumber,

    /// [`sov_rollup_interface::stf::ExecutionContext`]
    pub execution_context: sov_rollup_interface::stf::ExecutionContext,

    /// Gas used during slot processing, expressed in gas units
    pub gas_used: Vec<u64>,
}

impl Metric for RunnerDaMetrics {
    fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
        let metadata = serialize_metadata();

        write!(
            buffer,
            "sov_rollup_runner_da{metadata} da_height={},sync_distance={},get_block_time_ms={}",
            self.da_height,
            self.sync_distance,
            self.get_block_time.as_millis(),
        )
    }
}

impl Metric for RunnerCountMetrics {
    fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
        let metadata = serialize_metadata();

        write!(
            buffer,
            "sov_rollup_runner_counts{metadata} da_height={},batches_c={},transactions_c={},proofs_c={},batch_bytes={},proof_bytes={}",
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
        let metadata = serialize_metadata();

        write!(
            buffer,
            "sov_rollup_runner_times_us{metadata} da_height={},process_slot={},apply_slot={},stf_transition={},extract_blobs={},blob_extraction_proof={}",
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
        let metadata = serialize_metadata();

        write!(buffer, "sov_rollup_transaction_execution_us,status={:?},context={:?},call_message={},sequencer={}{metadata} value={},rollup_height={}",
               // tags
               self.tx_effect,
               self.execution_context,
               self.call_message,
               self.sequencer_address,
               //fields
               self.execution_time.as_micros(),
               self.visible_slot_number,
        )
    }
}

impl Metric for SlotProcessingMetrics {
    fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
        let metadata = serialize_metadata();

        write!(
            buffer,
            "sov_rollup_slot_execution_time_us,context={:?}{metadata} blobs_selection={},finalization={},visible_slot_number={},da_height={}",
            // Tags
            self.execution_context,
            // Fields
            self.blobs_selection_time.as_micros(),
            self.slot_finalization_time.as_micros(),
            self.visible_slot_number,
            self.da_height,
        )?;
        if self.gas_used.len() >= 2 {
            write!(
                buffer,
                ",gas_used_compute={},gas_used_mem={}",
                self.gas_used[0], self.gas_used[1],
            )?;
        }
        Ok(())
    }
}

impl Metric for UserSpaceSlotProcessingMetrics {
    fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
        let metadata = serialize_metadata();

        write!(
            buffer,
            "sov_rollup_slot_execution_time_us,context={:?}{metadata} begin_hooks={},blobs_processing={},end_hooks={},rollup_height={}",
            // Tags
            self.execution_context,
            // Fields
            self.begin_block_hook_time.as_micros(),
            self.blobs_processing_time.as_micros(),
            self.end_block_hook_time.as_micros(),
            self.visible_slot_number,
        )?;
        if self.gas_used.len() >= 2 {
            write!(
                buffer,
                ",gas_used_compute={},gas_used_mem={}",
                self.gas_used[0], self.gas_used[1],
            )?;
        }
        Ok(())
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
#[derive(Debug)]
pub struct BatchMetrics {
    #[allow(missing_docs)]
    pub processing_time: std::time::Duration,
    /// Number of transactions have been processed in batch.
    pub transactions_count: usize,
    /// Number of transactions have been ignored..
    pub ignored_transactions_count: usize,
}

impl Metric for BatchMetrics {
    fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
        let metadata = serialize_metadata();

        write!(
            buffer,
            "sov_rollup_batch_processing{metadata} processing_time_us={},transactions={},ignored_transactions={}",
            self.processing_time.as_micros(),
            self.transactions_count,
            self.ignored_transactions_count,
        )
    }
}

/// Metrics for an HTTP subsystem.
/// Can be applied to REST API or JSON RPC.
#[derive(Debug)]
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
        let metadata = serialize_metadata();

        write!(
            buffer,
            "sov_rollup_http_handlers,req_method={},resp_status={},path={}{metadata} processing_time_us={},response_body_bytes={}",
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

/// Representation of cycle count and free heap for a particular chunk of execution inside ZK VM guest.
#[derive(Debug)]
pub struct ZkVmExecutionChunk {
    /// Name of the caller site, usually a function or method
    pub name: String,
    /// Metadata associated with the metric. Usually input values collected from the caller function
    pub metadata: Vec<(String, String)>,
    /// A number of ZKVM cycles have been spent on this call.
    pub cycles_count: u64,
    /// Available bytes on the heap after execution of the block is complete.
    pub free_heap_bytes: u64,
    /// Amount of bytes of memory used during the execution of the block (and not reclaimed after execution is complete).
    pub memory_used: u64,
}

impl Metric for ZkVmExecutionChunk {
    fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
        // We are adding the metadata as measurmement tags in the influxdb line protocol.
        let metadata = if !self.metadata.is_empty() {
            let zk_metadata = self
                .metadata
                .iter()
                .map(|(key, value)| {
                    // Uses special telegraf formatting
                    let telegraf_formatted_key = safe_telegraf_string(key);

                    format!("{}={}", telegraf_formatted_key, value)
                })
                .collect::<Vec<_>>()
                .join(",");

            format!(",{zk_metadata}{}", serialize_metadata())
        } else {
            serialize_metadata()
        };

        write!(
            buffer,
            "sov_rollup_zkvm,name={}{metadata} cycles_count={},free_heap_bytes={},memory_used={}",
            self.name, self.cycles_count, self.free_heap_bytes, self.memory_used
        )
    }
}

#[derive(Debug)]
#[allow(missing_docs)]
pub enum ZkCircuit {
    Inner,
    Outer,
}

/// How much wall clock time it took to run prover
#[derive(Debug)]
pub struct ZkProvingTime {
    #[allow(missing_docs)]
    pub proving_time: std::time::Duration,
    #[allow(missing_docs)]
    pub is_success: bool,
    #[allow(missing_docs)]
    pub zk_circuit: ZkCircuit,
}

impl Metric for ZkProvingTime {
    fn serialize_for_telegraf(&self, buffer: &mut Vec<u8>) -> std::io::Result<()> {
        let metadata = serialize_metadata();

        write!(
            buffer,
            "sov_rollup_zkvm_proving,is_success={},circuit={:?}{metadata} proving_time_ms={}",
            self.is_success,
            self.zk_circuit,
            self.proving_time.as_millis()
        )
    }
}

#[derive(strum::EnumDiscriminants, Debug)]
#[strum_discriminants(
    derive(strum::EnumString),
    name(SovRollupMetrics),
    vis(pub),
    strum(serialize_all = "snake_case")
)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum SovRollupMetric {
    /// Metrics for the Da layer.
    RunnerDa(Timestamp, RunnerDaMetrics),
    /// Metrics to track the number of transactions, batches and slots processed inside the runner.
    RunnerCount(Timestamp, RunnerCountMetrics),
    /// Metrics to track the time spent on processing transactions, batches and slots inside the runner.
    RunnerTimes(Timestamp, RunnerTimeMetrics),
    /// Metrics to track the time spent on processing slots.
    SlotProcessing(Timestamp, SlotProcessingMetrics),
    /// Metrics to track user space slot processing.
    UserSpaceSlotProcessing(Timestamp, UserSpaceSlotProcessingMetrics),
    /// Metrics to track batch processing.
    BatchProcessing(Timestamp, BatchMetrics),
    /// Metrics to track transaction processing.
    TransactionProcessing(Timestamp, TransactionProcessingMetrics),
    /// Metrics to track HTTP requests.
    Http(Timestamp, HttpMetrics),
    /// Metrics to track ZKVM execution.
    ZkVm(Timestamp, ZkVmExecutionChunk),
    /// Metrics to track ZK proving.
    ZkProving(Timestamp, ZkProvingTime),
    #[cfg(feature = "gas-constant-estimation")]
    /// Metrics to track gas constant usage.
    GasConstantUsage(Timestamp, GasConstantMetric),
    /// Any custom metric can be developed externally
    Custom(Timestamp, Box<dyn Metric>),
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
            SovRollupMetric::ZkProving(t, m) => {
                m.serialize_for_telegraf(buffer)?;
                t
            }
            #[cfg(feature = "gas-constant-estimation")]
            SovRollupMetric::GasConstantUsage(t, m) => {
                m.serialize_for_telegraf(buffer)?;
                t
            }
            SovRollupMetric::Custom(t, m) => {
                m.serialize_for_telegraf(buffer)?;
                t
            }
        };

        write!(buffer, " {timestamp}")
    }
}

pub(crate) fn serialize_metadata() -> String {
    let metadata = METRICS_METADATA.read().unwrap();

    let metadata_string = metadata
        .iter()
        .map(|(key, value)| {
            let telegraf_key = safe_telegraf_string(key);
            let telegraf_value = safe_telegraf_string(value);
            format!("{telegraf_key}={telegraf_value}")
        })
        .collect::<Vec<_>>()
        .join(",");

    if !metadata_string.is_empty() {
        format!(",{metadata_string}")
    } else {
        String::new()
    }
}
