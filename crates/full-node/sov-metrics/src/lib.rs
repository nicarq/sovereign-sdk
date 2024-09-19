//! Prometheus metrics for Sovereign rollups.

#![deny(missing_docs)]

use std::sync::OnceLock;

use prometheus::{
    register_histogram, register_int_counter_with_registry, register_int_gauge_with_registry,
    Histogram, IntCounter, IntGauge,
};

/// Applies a function to the global [`Metrics`] instance if and only if the
/// `native` feature is enabled.
pub fn update_metrics<F>(f: F)
where
    F: FnOnce(&Metrics),
{
    if cfg!(feature = "native") {
        static METRICS: OnceLock<Metrics> = OnceLock::new();

        f(OnceLock::get_or_init(&METRICS, || {
            tracing::info!("Registering rollup metrics with prometheus");
            Metrics::new(prometheus::default_registry())
                .expect("failed to create new metrics; this is a bug in the Sovereign SDK")
        }));
    }
}

/// Prometheus metrics for Sovereign rollups.
///
/// Values of this type are only accessible through the [`update_metrics`] function.
#[derive(Debug)]
pub struct Metrics {
    /// Number of DA blocks processed.
    pub da_blocks_processed: IntCounter,
    /// Number of rollup batches processed.
    pub rollup_batches_processed: IntCounter,
    /// The total size (in bytes) of all batches processed to date.
    ///
    /// Note that this metric only tracks the size of the batches which have been ingested
    /// by the STF. If the rollup does some internal reordering of the batches, this metric will not
    /// reflect which blobs have and have not been executed.
    pub batch_bytes_processed: IntCounter,
    /// Number of rollup transactions processed.
    pub rollup_txns_processed: IntCounter,
    /// Number of proof blobs processed.
    pub proof_blobs_processed: IntCounter,
    /// The total size (in bytes) of all proofs processed to date.
    ///
    /// Note that this metric only tracks the size of the proofs which have been ingested
    /// by the STF. If the rollup does some internal reordering of the proofs, this metric will not
    /// reflect which proofs have and have not been verified.
    pub proof_bytes_processed: IntCounter,
    /// Number of rollup transactions per DA block.
    pub rollup_txns_per_da_block: IntGauge,
    /// Current DA height for the rollup.
    pub current_da_height: IntGauge,
    /// Distance from current Rollup Height to Da height.
    pub sync_distance: IntGauge,
    /// Full time it took to process slot. Includes all operations required together with pre/post processing.
    pub process_slot_sec: Histogram,
    /// Time it took to execute only STF transition.
    pub stf_transition_sec: Histogram,
    /// Time it took to get a block from DaService.
    pub get_block_sec: Histogram,
    /// Full time it took to process slot. Includes all operations required together with pre/post processing.
    pub process_slot_ms_by_slot: IntGauge,
    /// Time it took to execute the STF transition, post-process, and commit all results to the DB.
    pub stf_transition_with_commit_ms_by_slot: IntGauge,
    /// Time it took to execute only STF transition without post-processing or committing to the DB.
    pub apply_slot_ms_by_slot: IntGauge,
    /// Time it took to get a block from DaService.
    pub get_block_ms_by_slot: IntGauge,
    /// Time it took to extract relevant blobs.
    pub extract_blobs_ms_by_slot: IntGauge,
    /// Time it took to prove the the relevant blobs were extracted correctly.
    pub get_blob_extraction_proof_ms_by_slot: IntGauge,
}

impl Metrics {
    fn new(registry: &prometheus::Registry) -> prometheus::Result<Self> {
        let da_blocks_processed = register_int_counter_with_registry!(
            "sov_da_blocks_processed_count",
            "Number of DA blocks processed",
            registry,
        )?;

        let rollup_batches_processed = register_int_counter_with_registry!(
            "sov_rollup_batches_processed_count",
            "Number of rollup batches processed",
            registry,
        )?;

        let batch_bytes_processed = register_int_counter_with_registry!(
            "sov_batch_bytes_processed",
            "Total size (in bytes) of all batches processed to date",
            registry,
        )?;

        let proof_blobs_processed = register_int_counter_with_registry!(
            "sov_proof_blobs_processed_count",
            "Number of proof blobs processed",
            registry,
        )?;

        let proof_bytes_processed = register_int_counter_with_registry!(
            "sov_proof_bytes_processed",
            "Total size (in bytes) of all proofs processed to date",
            registry,
        )?;

        let rollup_txns_processed = register_int_counter_with_registry!(
            "sov_rollup_txns_processed_count",
            "Number of rollup transactions processed",
            registry,
        )?;

        let rollup_txns_per_da_block = register_int_gauge_with_registry!(
            "sov_rollup_txns_per_da_block",
            "Number of rollup transactions per DA block",
            registry,
        )?;

        let current_da_height = register_int_gauge_with_registry!(
            "sov_current_da_height",
            "Current DA height for the rollup",
            registry,
        )?;

        let sync_distance = register_int_gauge_with_registry!(
            "sov_sync_distance",
            "Distance from current Rollup Height to DA height",
            registry
        )?;

        let process_slot_sec = register_histogram!(
            "sov_process_slot_sec",
            "Time took to fully execute slot",
            vec![0.05, 0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 20.0, 30.0, 60.0, 120.0, 300.0, 600.0],
        )?;

        let stf_transition_sec = register_histogram!(
            "sov_stf_transition_sec",
            "Time took only to execute STF",
            vec![0.05, 0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 20.0, 30.0, 60.0, 120.0, 300.0, 600.0],
        )?;

        let get_block_sec = register_histogram!(
            "sov_get_block_sec",
            "Time took only to get block from DA",
            vec![0.05, 0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 20.0, 30.0, 60.0, 120.0, 300.0, 600.0],
        )?;

        let process_slot_ms_by_slot = register_int_gauge_with_registry!(
            "sov_process_slot_ms_by_slot",
            "The time taken to process each slot, including fetching from DA and storing the output to DB",
            registry,
        )?;

        let stf_transition_with_commit_ms_by_slot = register_int_gauge_with_registry!(
            "sov_stf_transition_with_commit_ms_by_slot",
            "The time taken from receiving the raw block from DA through STF transition and committing to storage",
            registry,
        )?;

        let apply_slot_ms_by_slot = register_int_gauge_with_registry!(
            "sov_apply_slot_ms_by_slot",
            "The time taken in the 'apply_slot' function only",
            registry,
        )?;

        let extract_blobs_ms_by_slot = register_int_gauge_with_registry!(
            "sov_extract_blobs_ms_by_slot",
            "The time taken in the 'extract_relevant_blobs' function only",
            registry,
        )?;

        let get_blob_extraction_proof_ms_by_slot = register_int_gauge_with_registry!(
            "sov_get_blob_extraction_proof_ms_by_slot",
            "The time taken in the 'get_extraction_proof' function only",
            registry,
        )?;

        let get_block_ms_by_slot = register_int_gauge_with_registry!(
            "sov_get_block_ms_by_slot",
            "The time taken to fetch the block from the DA layer",
            registry,
        )?;

        Ok(Self {
            da_blocks_processed,
            rollup_batches_processed,
            batch_bytes_processed,
            rollup_txns_processed,
            proof_blobs_processed,
            proof_bytes_processed,
            rollup_txns_per_da_block,
            current_da_height,
            sync_distance,
            process_slot_sec,
            stf_transition_sec,
            get_block_sec,
            process_slot_ms_by_slot,
            stf_transition_with_commit_ms_by_slot,
            apply_slot_ms_by_slot,
            get_block_ms_by_slot,
            extract_blobs_ms_by_slot,
            get_blob_extraction_proof_ms_by_slot,
        })
    }
}
