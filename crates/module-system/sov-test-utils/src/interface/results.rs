use sov_modules_api::{Spec, TxEffect};
use sov_modules_stf_blueprint::TxReceiptContents;

/// The expected outcome of a batch.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BatchExpectedReceipt<S: Spec> {
    /// The list of [`TxEffect`] for each transaction executed in the batch
    pub(crate) tx_receipts: Vec<TxEffect<TxReceiptContents<S>>>,
    /// The expected outcome of the batch
    pub(crate) batch_outcome: sov_modules_api::BatchSequencerOutcome,
}

/// Defines the expected receipts of a slot. This is simply a list of [`BatchExpectedReceipt`]s.
pub type SlotExpectedReceipt<S> = Vec<BatchExpectedReceipt<S>>;
