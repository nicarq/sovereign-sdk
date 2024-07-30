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
    /// Number of rollup transactions processed.
    pub rollup_txns_processed: IntCounter,
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
}

impl Metrics {
    fn new(registry: &prometheus::Registry) -> prometheus::Result<Self> {
        let da_blocks_processed = register_int_counter_with_registry!(
            "da_blocks_processed_count",
            "Number of DA blocks processed",
            registry,
        )?;

        let rollup_batches_processed = register_int_counter_with_registry!(
            "rollup_batches_processed_count",
            "Number of rollup batches processed",
            registry,
        )?;

        let rollup_txns_processed = register_int_counter_with_registry!(
            "rollup_txns_processed_count",
            "Number of rollup transactions processed",
            registry,
        )?;

        let rollup_txns_per_da_block = register_int_gauge_with_registry!(
            "rollup_txns_per_da_block",
            "Number of rollup transactions per DA block",
            registry,
        )?;

        let current_da_height = register_int_gauge_with_registry!(
            "current_da_height",
            "Current DA height for the rollup",
            registry,
        )?;

        let sync_distance = register_int_gauge_with_registry!(
            "sync_distance",
            "Distance from current Rollup Height to DA height",
            registry
        )?;

        let process_slot_sec = register_histogram!(
            "process_slot_sec",
            "Time took to fully execute slot",
            vec![0.05, 0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 20.0, 30.0, 60.0, 120.0, 300.0, 600.0],
        )?;

        let stf_transition_sec = register_histogram!(
            "stf_transition_sec",
            "Time took only to execute STF",
            vec![0.05, 0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 20.0, 30.0, 60.0, 120.0, 300.0, 600.0],
        )?;

        let get_block_sec = register_histogram!(
            "get_block_sec",
            "Time took only to get block from DA",
            vec![0.05, 0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 20.0, 30.0, 60.0, 120.0, 300.0, 600.0],
        )?;

        Ok(Self {
            da_blocks_processed,
            rollup_batches_processed,
            rollup_txns_processed,
            rollup_txns_per_da_block,
            current_da_height,
            sync_distance,
            process_slot_sec,
            stf_transition_sec,
            get_block_sec,
        })
    }
}
