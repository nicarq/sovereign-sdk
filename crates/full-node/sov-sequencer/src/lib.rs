#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

pub mod batch_builders;
mod config;
mod rest_api;
mod sequencer;
mod spec;
mod tx_status;

use batch_builders::BatchBuilder;
pub use config::{BatchBuilderConfig, BatchBuilderMode, SequencerConfig};
pub use sequencer::Sequencer;
use serde::{Deserialize, Serialize};
pub use sov_db::sequencer_db::{SeqDbTx, SeqDbTxId, SequencerDb};
use sov_modules_api::FullyBakedTx;
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

/// Extends [`SeqDbTx`] with methods that require [`sov_sequencer`](crate)-specific types.
pub trait SeqDbTxExtend {
    /// Creates a new [`SeqDbTx`] from a [`TxHash`] and [`BatchBuilder::TxInput`].
    fn new<Bb: BatchBuilder>(tx_hash: TxHash, tx_input: Bb::TxInput) -> Self;

    /// Returns the fully encoded transaction stored in the [`SeqDbTx`].
    fn fully_baked_tx(&self) -> FullyBakedTx;

    /// Returns the [`BatchBuilder::TxInput`] for the transaction stored in the [`SeqDbTx`].
    fn tx_input<Bb: BatchBuilder>(&self) -> Bb::TxInput;
}

impl SeqDbTxExtend for SeqDbTx {
    fn new<Bb: BatchBuilder>(tx_hash: TxHash, tx_input: Bb::TxInput) -> Self {
        Self::new_with_tx_bytes(
            tx_hash,
            borsh::to_vec(&tx_input).expect("Failed to serialize transaction in SeqDbTx::new"),
        )
    }

    fn fully_baked_tx(&self) -> FullyBakedTx {
        FullyBakedTx::new(self.tx_bytes.clone())
    }

    fn tx_input<Bb: BatchBuilder>(&self) -> Bb::TxInput {
        borsh::from_slice(&self.tx_bytes).expect(
            "Failed to deserialize stored transaction; db data is corrupted or there's a bug",
        )
    }
}
