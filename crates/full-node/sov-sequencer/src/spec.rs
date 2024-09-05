use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use sov_modules_api::TxReceiptContents;
use sov_rollup_interface::node::batch_builder::BatchBuilder;
use sov_rollup_interface::node::da::DaService;

/// A bunch of associated types that define the behavior of a
/// [`Sequencer`](crate::Sequencer).
pub trait SequencerSpec: Clone + Send + Sync + 'static {
    /// The [`BatchBuilder`] that the sequencer uses to process submitted
    /// transactions and assemble them into batches.
    type BatchBuilder: BatchBuilder;
    /// The [`DaService`] that the sequencer uses to communicate with the DA
    /// layer.
    type Da: DaService;
    /// The type of the batch receipt that the rollup stores in
    /// [`sov_db::ledger_db::LedgerDb`].
    type BatchReceipt: DeserializeOwned + Send + Sync;
    /// The type of the transaction receipt that the rollup stores in the
    /// [`sov_db::ledger_db::LedgerDb`].
    type TxReceipt: TxReceiptContents;
}

/// A [`SequencerSpec`] with explicit generic types.
#[derive(derivative::Derivative)]
#[derivative(Clone(bound = ""))]
pub struct GenericSequencerSpec<B, Da, BatchReceipt, TxReceipt>(
    PhantomData<(B, Da, BatchReceipt, TxReceipt)>,
);

impl<B, Da, BatchReceipt, TxReceipt> SequencerSpec
    for GenericSequencerSpec<B, Da, BatchReceipt, TxReceipt>
where
    B: BatchBuilder,
    Da: DaService,
    BatchReceipt: DeserializeOwned + Send + Sync + 'static,
    TxReceipt: TxReceiptContents,
{
    type BatchBuilder = B;
    type Da = Da;
    type BatchReceipt = BatchReceipt;
    type TxReceipt = TxReceipt;
}
