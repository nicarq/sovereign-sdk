#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

pub mod batch_builders;
mod config;
mod db;
mod drop_notifier;
mod rest_api;
mod sequencer;
mod spec;
mod tx_status;

pub use config::{BatchBuilderConfig, SequencerConfig};
pub use db::{SeqDbTx, SequencerDb};
pub use sequencer::Sequencer;
use serde::{Deserialize, Serialize};
use sov_rollup_interface::TxHash;
pub use spec::{GenericSequencerSpec, SequencerSpec};
pub use tx_status::TxStatusManager;

pub use crate::tx_status::TxStatus;

/// The response type to REST API calls that successfully publish a batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmittedBatchInfo {
    /// The DA height for which the batch was submitted.
    pub da_height: u64,
    /// The number of transactions that were successfully included in the batch.
    pub num_txs: usize,
}
