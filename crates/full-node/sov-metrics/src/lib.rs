//! Metric tracking for Sovereign rollups.
#![deny(missing_docs)]

mod influxdb;
mod prometheus;

#[cfg(feature = "native")]
pub use influxdb::{
    init_metrics_tracker, track_metrics, MetricsTracker, MonitoringConfig, RunnerMetrics,
};
#[cfg(not(feature = "native"))]
pub use influxdb::{track_metrics, MetricsTracker};
pub use prometheus::{update_metrics, Metrics};
