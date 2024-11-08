use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use sov_modules_api::{Spec, StoredEvent, TxReceiptContents};
use sov_rollup_interface::node::da::DaService;

use crate::batch_builders::BatchBuilder;

/// A bunch of associated types that define the behavior of a
/// [`Sequencer`](crate::Sequencer).
pub trait SequencerSpec: Clone + Send + Sync + 'static {
    /// The [`BatchBuilder`] that the sequencer uses to process submitted
    /// transactions and assemble them into batches.
    type BatchBuilder: BatchBuilder;
    /// The [`DaService`] that the sequencer uses to communicate with the DA
    /// layer.
    ///
    /// Its [`DaSpec`](sov_modules_api::DaSpec) **MUST** be the same one
    /// specified by [`SequencerSpec::BatchBuilder`].
    type Da: DaService<Spec = <<Self::BatchBuilder as BatchBuilder>::Spec as Spec>::Da>;
    /// The type of the batch receipt that the rollup stores in
    /// [`sov_db::ledger_db::LedgerDb`].
    type BatchReceipt: DeserializeOwned + Send + Sync;
    /// The type of the transaction receipt that the rollup stores in the
    /// [`sov_db::ledger_db::LedgerDb`].
    type TxReceipt: TxReceiptContents;
    /// The type of the events that the rollup stores in the
    /// [`sov_db::ledger_db::LedgerDb`].
    type Event: TryFrom<(u64, StoredEvent), Error = anyhow::Error> + Send + Sync + DeserializeOwned;
}

/// A [`SequencerSpec`] with explicit generic types.
#[derive(derivative::Derivative)]
#[derivative(Clone(bound = ""))]
pub struct GenericSequencerSpec<B, Da, BatchReceipt, TxReceipt, Event>(
    PhantomData<(B, Da, BatchReceipt, TxReceipt, Event)>,
);

impl<B, Da, BatchReceipt, TxReceipt, Event> SequencerSpec
    for GenericSequencerSpec<B, Da, BatchReceipt, TxReceipt, Event>
where
    B: BatchBuilder,
    Da: DaService<Spec = <<B as BatchBuilder>::Spec as Spec>::Da>,
    BatchReceipt: DeserializeOwned + Send + Sync + 'static,
    TxReceipt: TxReceiptContents,
    Event: TryFrom<(u64, StoredEvent), Error = anyhow::Error>
        + Send
        + Sync
        + DeserializeOwned
        + 'static,
{
    type BatchBuilder = B;
    type Da = Da;
    type BatchReceipt = BatchReceipt;
    type TxReceipt = TxReceipt;
    type Event = Event;
}
