use serde::{Deserialize, Serialize};

/// Cycle utils for the risc0 zkvm
pub mod risc0;

/// Cycle utils for the sp1 zkvm
pub mod sp1;

/// Metric to report to the zkvm
#[derive(Debug, Serialize, Deserialize)]
pub struct CycleMetric {
    /// Identifier of the tagged function
    pub name: String,
    /// Metadata to include with the metric
    pub metadata: Vec<(String, String)>,
    /// Number of cycles
    pub count: u64,
    /// Free heap bytes
    pub free_heap_bytes: u64,
}

#[cfg(feature = "native")]
/// Deserialize the output of the metrics syscall
pub fn deserialize_metrics_call(serialized: &[u8]) -> anyhow::Result<CycleMetric> {
    Ok(bincode::deserialize::<CycleMetric>(serialized)?)
}
