use sov_rollup_interface::da::{BlobReaderTrait, DaSpec};

use crate::{KernelStateAccessor, Spec};

/// The namespace in which a blob appeared.
#[derive(Debug, PartialEq, Eq, Hash)]
pub enum BlobOrigin<'a, T> {
    /// The blob is from the "batch" namespace. These blobs contain transactions.
    Batch(&'a mut T),
    /// The blob is from the "proof" namespace. These blobs contain proofs.
    Proof(&'a mut T),
}

impl<'a, T: BlobReaderTrait> BlobOrigin<'a, T> {
    /// Returns the total number of bytes in the blob.
    pub fn total_len(&self) -> usize {
        match self {
            BlobOrigin::Batch(b) => b.total_len(),
            BlobOrigin::Proof(p) => p.total_len(),
        }
    }
}

/// The type of the sequencer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SequencerType<S: Spec> {
    /// Preferred sequencer.
    Preferred(<<S as Spec>::Da as DaSpec>::Address),
    /// Standard sequencer.
    Standard(<<S as Spec>::Da as DaSpec>::Address),
}

impl<S: Spec> SequencerType<S> {
    /// The sequencer DA address.
    pub fn address(&self) -> &<<S as Spec>::Da as DaSpec>::Address {
        match self {
            SequencerType::Preferred(address) | SequencerType::Standard(address) => address,
        }
    }
}

/// Output of the [`BlobSelector::get_blobs_for_this_slot`] method from the [`BlobSelector`] trait.
pub struct BlobSelectorOutput<S: Spec, BlobType> {
    /// The blobs selected by the module.
    pub selected_blobs: Vec<(BlobType, SequencerType<S>)>,
    /// Whether the slot hooks should be executed. We should execute slot hooks whenever we accept blobs for execution
    /// or when we increase the visible slot number.
    pub should_execute_slot_hooks: bool,
}

impl<S: Spec, B> BlobSelectorOutput<S, B> {
    /// Apply the given function to each blob
    pub fn map_blobs<Target>(
        self,
        mut f: impl FnMut(B) -> Target,
    ) -> BlobSelectorOutput<S, Target> {
        BlobSelectorOutput {
            selected_blobs: self
                .selected_blobs
                .into_iter()
                .map(|(batch, sender)| (f(batch), sender))
                .collect(),
            should_execute_slot_hooks: self.should_execute_slot_hooks,
        }
    }
}

/// BlobSelector decides which blobs to process in a current slot.
pub trait BlobSelector {
    /// Spec type
    type Spec: Spec;

    /// The type of batch returned by the selector
    type BlobType;

    /// Whether the kernel accepts "preferred" batches in a special format.
    const ACCEPTS_PREFERRED_BATCHES: bool;

    /// Returns a vector of blobs that should be processed in the current slot.
    #[allow(clippy::type_complexity)]
    fn get_blobs_for_this_slot<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelStateAccessor<'k, <Self::Spec as Spec>::Storage>,
    ) -> anyhow::Result<BlobSelectorOutput<Self::Spec, Self::BlobType>>
    where
        I: IntoIterator<
            Item = BlobOrigin<'a, <<Self::Spec as Spec>::Da as DaSpec>::BlobTransaction>,
        >;

    /// Implementors that don't support preferred blobs SHOULD panic.
    fn next_sequence_number(
        &self,
        _state: &mut KernelStateAccessor<'_, <Self::Spec as Spec>::Storage>,
    ) -> u64 {
        panic!("Kernel does not support preferred blobs. Please change kernel type.")
    }
}
