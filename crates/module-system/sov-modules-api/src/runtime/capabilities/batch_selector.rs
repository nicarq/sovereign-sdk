use sov_rollup_interface::da::DaSpec;

use crate::{KernelStateAccessor, Spec};

/// The namespace in which a blob appeared.
#[derive(Debug, PartialEq, Eq, Hash)]
pub enum BlobOrigin<'a, T> {
    /// The blob is from the "batch" namespace. These blobs contain transactions.
    Batch(&'a mut T),
    /// The blob is from the "proof" namespace. These blobs contain proofs.
    Proof(&'a mut T),
}

/// Output of the [`BlobSelector::get_blobs_for_this_slot`] method from the [`BlobSelector`] trait.
pub struct BlobSelectorOutput<S: Spec, BlobType> {
    /// The blobs selected by the module.
    pub selected_blobs: Vec<(BlobType, <S::Da as DaSpec>::Address)>,
    /// Whether the slot hooks should be executed. We should execute slot hooks whenever we accept blobs for execution
    /// or when we increase the virtual slot number.
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
}
