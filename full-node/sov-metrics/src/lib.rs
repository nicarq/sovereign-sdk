//! Feature gated metrics functions for capturing rollup specific metrics

#![deny(missing_docs)]

/// Lazy static counters registered with the prometheus crateg
#[cfg(feature = "native")]
pub mod counters;
#[cfg(feature = "native")]
use counters::*;

#[cfg(feature = "native")]
#[inline(always)]
/// Increment the counter `DA_BLOCKS_PROCESSED` by count
/// if metrics are enabled. No-op otherwise
pub fn inc_da_blocks_processed(count: usize) {
    DA_BLOCKS_PROCESSED.inc_by(count as u64)
}

#[cfg(not(feature = "native"))]
#[inline(always)]
/// Increment the counter `DA_BLOCKS_PROCESSED` by count
/// if metrics are enabled. No-op otherwise
pub fn inc_da_blocks_processed(_count: usize) {}

#[cfg(feature = "native")]
#[inline(always)]
/// Increment the counter `ROLLUP_BATCHES_PROCESSED` by count
/// if metrics are enabled. No-op otherwise
pub fn inc_rollup_batches_processed(count: usize) {
    ROLLUP_BATCHES_PROCESSED.inc_by(count as u64)
}

#[cfg(not(feature = "native"))]
#[inline(always)]
/// Increment the counter `ROLLUP_BATCHES_PROCESSED` by count
/// if metrics are enabled. No-op otherwise
pub fn inc_rollup_batches_processed(_count: usize) {}

#[cfg(feature = "native")]
#[inline(always)]
/// Increment the counter `ROLLUP_TRANSACTIONS_PROCESSED` by count
/// if metrics are enabled. No-op otherwise
pub fn inc_rollup_transactions_processed(count: usize) {
    ROLLUP_TRANSACTIONS_PROCESSED.inc_by(count as u64)
}

#[cfg(not(feature = "native"))]
#[inline(always)]
/// Increment the counter `ROLLUP_TRANSACTIONS_PROCESSED` by count
/// if metrics are enabled. No-op otherwise
pub fn inc_rollup_transactions_processed(_count: usize) {}

#[cfg(feature = "native")]
#[inline(always)]
/// Set the gauge `ROLLUP_TRANSACTIONS_PER_DA_BLOCK` by count
/// if metrics are enabled. No-op otherwise
pub fn set_rollup_transactions_processed(num_txns: usize) {
    ROLLUP_TRANSACTIONS_PER_DA_BLOCK.set(num_txns as i64)
}

#[cfg(not(feature = "native"))]
#[inline(always)]
/// Set the gauge `ROLLUP_TRANSACTIONS_PER_DA_BLOCK` by count
/// if metrics are enabled. No-op otherwise
pub fn set_rollup_transactions_processed(_num_txns: usize) {}

#[cfg(feature = "native")]
#[inline(always)]
/// Set the gauge `CURRENT_DA_HEIGHT` by count
/// if metrics are enabled. No-op otherwise
pub fn set_current_da_height(count: u64) {
    CURRENT_DA_HEIGHT.set(count as i64)
}

#[cfg(not(feature = "native"))]
#[inline(always)]
/// Set the gauge `CURRENT_DA_HEIGHT` by count
/// if metrics are enabled. No-op otherwise
pub fn set_current_da_height(_count: u64) {}
