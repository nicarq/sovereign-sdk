#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

mod batch_builder;
mod db;
mod mempool;
mod rest_api;
mod sequencer;
mod tx_status;

pub use batch_builder::{FairBatchBuilder, FairBatchBuilderConfig};
pub use db::{MempoolTx, SequencerDb};
pub use rest_api::sequencer_rest_api_server;
pub use sequencer::{GenericSequencerSpec, Sequencer, SequencerSpec};
use serde::{Deserialize, Serialize};
use sov_rollup_interface::TxHash;
pub use tx_status::TxStatusManager;

pub use crate::tx_status::TxStatus;

/// The response type to REST API calls that successfully publish a batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmittedBatchInfo {
    /// The DA height for which the batch was submitted.
    pub da_height: u64,
    /// The number of transactions that were successfully included in the batch.
    pub num_txs: usize,
}
