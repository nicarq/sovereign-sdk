#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

pub mod batch_builders;
mod config;
mod rest_api;
mod sequencer;
mod spec;
mod tx_status;

use batch_builders::BatchBuilder;
pub use config::{BatchBuilderConfig, SequencerConfig};
pub use sequencer::Sequencer;
use serde::Serialize;
pub use sov_db::sequencer_db::{SeqDbTx, SeqDbTxId, SequencerDb};
use sov_modules_api::{DaSpec, FullyBakedTx};
use sov_rollup_interface::node::da::SubmitBlobReceipt;
use sov_rollup_interface::TxHash;
pub use spec::{GenericSequencerSpec, SequencerSpec};
pub use tx_status::TxStatusManager;

pub use crate::tx_status::TxStatus;

/// The response type to REST API calls that successfully publish a batch.
#[derive(Debug, Clone, Serialize)]
pub struct SubmitBatchReceipt<Da: DaSpec> {
    /// All the hashes of the transactions that were successfully included in
    /// the batch.
    pub tx_hashes: Vec<TxHash>,
    /// Blob metadata to track its status.
    #[serde(flatten)]
    pub submit_blob_receipt: SubmitBlobReceipt<Da::TransactionId>,
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
