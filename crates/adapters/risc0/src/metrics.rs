//! Defines utilities for collecting runtime metrics from inside a Risc0 VM
use anyhow::Context;
use risc0_zkvm::Bytes;
/// The name of the syscall we use to collect metrics from the Risc0 VM.
pub use sov_cycle_utils::risc0::SYSCALL_NAME_METRICS;

/// Deserialize a `Bytes` into a null-separated `(String, u64)` tuple. This function
/// expects its arguments to match the format of arguments to Risc0's io callbacks.
fn deserialize_custom(serialized: Bytes) -> anyhow::Result<(String, u64)> {
    let null_pos = serialized
        .iter()
        .position(|&b| b == 0)
        .context("Could not find separator in provided bytes")?;
    let (string_bytes, size_bytes_with_null) = serialized.split_at(null_pos);
    let size_bytes = &size_bytes_with_null[1..]; // Skip the null terminator
    let string = String::from_utf8(string_bytes.to_vec())?;
    let size = u64::from_le_bytes(size_bytes.try_into()?); // Convert bytes back into usize
    Ok((string, size))
}

/// A custom callback for extracting metrics from the Risc0 zkvm.
///
/// When the "bench" feature is enabled, this callback is registered as a syscall
/// in the Risc0 VM and invoked whenever a function annotated with the [`risc0-cycle-utils::cycle_tracker`]
/// macro is invoked.
pub fn metrics_callback(input: Bytes) -> anyhow::Result<Bytes> {
    let (metric, cycles_count) = deserialize_custom(input)?;
    sov_cycle_utils::increment_metric(metric.clone(), cycles_count);
    sov_metrics::track_metrics(|tracker| {
        tracker.track_zkvm_metric(sov_metrics::ZkVmCycleCount {
            name: metric,
            cycles_count,
        });
    });
    Ok(Bytes::new())
}
