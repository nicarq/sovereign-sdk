use sov_rollup_interface::da::DaSpec;

use crate::{KernelWorkingSet, Spec};

/// BatchSelector decides which batches to process in a current slot.
pub trait BatchSelector<Da: DaSpec> {
    /// Spec type
    type Spec: Spec;

    /// The type of batch returned by the selector
    type Batch;

    /// It takes two arguments.
    /// 1. `current_blobs` - blobs that were received from the network for the current slot.
    /// 2. `state` - the working to access storage.
    /// It returns a vector containing a mix of borrowed and owned blobs.
    fn get_batches_for_this_slot<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelWorkingSet<'k, Self::Spec>,
    ) -> anyhow::Result<Vec<(Self::Batch, Da::Address)>>
    where
        I: IntoIterator<Item = &'a mut Da::BlobTransaction>;
}
