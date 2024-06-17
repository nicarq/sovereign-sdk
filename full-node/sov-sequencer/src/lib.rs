#![deny(missing_docs)]
// Needed to allow nested `Arc`s.
#![allow(clippy::redundant_allocation)]
#![doc = include_str!("../README.md")]

mod batch_builder;
mod db;
mod mempool;
mod sequencer;
mod tx_status;
pub mod utils;

pub use batch_builder::{FairBatchBuilder, FairBatchBuilderConfig};
pub use db::{MempoolTx, SequencerDb};
pub use sequencer::Sequencer;
use serde::{Deserialize, Serialize};
use sov_rollup_interface::common::HexHash;
use sov_rollup_interface::services::batch_builder::TxHash;

pub use crate::tx_status::TxStatus;

/// The return type of `sequencer_publishBatch`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmittedBatchInfo {
    /// The DA height for which the batch was submitted.
    pub da_height: u64,
    /// The number of transactions that were successfully included in the batch.
    pub num_txs: usize,
}

/// The response type to the RPC method `sequencer_publishBatch`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishBatchResponse {
    /// Summary information about the batch submission result.
    batch: Result<SubmittedBatchInfo, String>,
    /// Detailed information about the contents of the batch that was submitted
    /// (or attempted to be submitted, if case of an error).
    accept_tx_results: Vec<AcceptTxResponse>,
}

/// The response type to the RPC method `sequencer_acceptTx`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptTxResponse {
    /// Raw transaction contents as originally passed by the client, as a
    /// hex-encoded string.
    #[serde(with = "sov_rollup_interface::common::hex_string_serde")]
    pub tx: Vec<u8>,
    /// The transaction hash of the transaction that was accepted.
    pub tx_hash: HexHash,
}
