//! Metric tracking for Sovereign rollups.
#![deny(missing_docs)]

/// Contains utilities to track zkvm cycles
pub mod cycle_utils;

#[cfg(feature = "native")]
mod influxdb;

#[cfg(feature = "native")]
pub use influxdb::{
    init_metrics_tracker, timestamp, track_metrics, BatchMetrics, BatchOutcome, HttpMetrics,
    MetricsTracker, MonitoringConfig, RunnerMetrics, SlotProcessingMetrics, SovRollupMetrics,
    TransactionEffect, TransactionProcessingMetrics, UserSpaceSlotProcessingMetrics, ZkCircuit,
    ZkProvingTime, ZkVmExecutionChunk,
};
#[cfg(all(feature = "native", feature = "gas-constant-estimation"))]
pub use influxdb::{GasConstantTracker, GAS_CONSTANTS};
