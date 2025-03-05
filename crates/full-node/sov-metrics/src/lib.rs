#![doc = include_str!("../README.md")]
#![deny(missing_docs)]

/// Contains utilities to track zkvm cycles
pub mod cycle_utils;

#[cfg(feature = "native")]
mod influxdb;

#[cfg(feature = "native")]
pub use influxdb::{
    init_metrics_tracker, timestamp, track_metrics, BatchMetrics, BatchOutcome, HttpMetrics,
    Metric, MetricsTracker, MonitoringConfig, RunnerMetrics, SlotProcessingMetrics,
    SovRollupMetrics, TelegrafSocketConfig, TransactionEffect, TransactionProcessingMetrics,
    UserSpaceSlotProcessingMetrics, ZkCircuit, ZkProvingTime, ZkVmExecutionChunk, METRICS_METADATA,
};
#[cfg(all(feature = "native", feature = "gas-constant-estimation"))]
pub use influxdb::{GasConstantTracker, GAS_CONSTANTS};

#[macro_export]
/// Starts a timer if the `native` feature is enabled. Otherwise, does nothing.
macro_rules! start_timer {
    ($timer:ident) => {
        #[cfg(feature = "native")]
        let $timer = std::time::Instant::now();
    };
}

#[macro_export]
/// Returns the elapsed time since the timer if the `native` feature is enabled. Otherwise does nothing.
macro_rules! save_elapsed {
    ($end:ident SINCE $start:ident) => {
        #[cfg(feature = "native")]
        let $end = $start.elapsed();
    };
}
