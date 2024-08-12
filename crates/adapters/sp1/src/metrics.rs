//! Defines utilities for collecting runtime metrics from inside a SP1 VM
use sp1_sdk::HookEnv;

/// A custom callback for extracting metrics from the SP1 zkvm.
///
/// When the "bench" feature is enabled, this callback is registered as a syscall
/// in the SP1 VM and invoked whenever a function annotated with the [`sp1-cycle-utils::cycle_tracker`]
/// macro is invoked.
pub fn metrics_hook(_env: HookEnv, buf: &[u8]) -> Vec<Vec<u8>> {
    let (cycles_buf, name_buf) = buf.split_at(std::mem::size_of::<u64>());
    let cycles = u64::from_le_bytes(cycles_buf.try_into().unwrap());
    let name = std::str::from_utf8(name_buf).unwrap();
    sov_cycle_utils::increment_metric(name.to_owned(), cycles);
    vec![]
}
