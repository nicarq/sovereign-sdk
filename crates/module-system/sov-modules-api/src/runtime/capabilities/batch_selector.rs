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
    ) -> anyhow::Result<
        Vec<(
            Self::BlobType,
            <<Self::Spec as Spec>::Da as DaSpec>::Address,
        )>,
    >
    where
        I: IntoIterator<
            Item = BlobOrigin<'a, <<Self::Spec as Spec>::Da as DaSpec>::BlobTransaction>,
        >;
}
