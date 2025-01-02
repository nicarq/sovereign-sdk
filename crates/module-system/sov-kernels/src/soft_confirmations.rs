//! The basic kernel provides censorship resistance by processing all blobs immediately in the order they appear on DA

use std::convert::Infallible;

use sov_blob_storage::BlobStorage;
use sov_chain_state::ChainState;
use sov_modules_api::capabilities::{BlobOrigin, BlobSelectorOutput};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::runtime::capabilities::{BlobSelector, Kernel};
use sov_modules_api::{
    BlobDataWithId, BootstrapWorkingSet, DaSpec, Gas, IterableBatchWithId, KernelStateAccessor,
    Spec, VersionReader, VisibleSlotNumber,
};
use sov_rollup_interface::common::SlotNumber;
use sov_state::Storage;

/// A kernel supporting based sequencing with soft confirmations
pub struct SoftConfirmationsKernel<'a, S: Spec> {
    pub chain_state: &'a ChainState<S>,
    pub blob_storage: &'a BlobStorage<S>,
}

impl<'a, S: Spec> Kernel<S> for SoftConfirmationsKernel<'a, S> {
    fn true_rollup_height(&self, state: &mut BootstrapWorkingSet<'_, S::Storage>) -> SlotNumber {
        self.chain_state
            .true_rollup_height(state)
            .unwrap_infallible()
    }
    fn next_visible_rollup_height(
        &self,
        state: &mut BootstrapWorkingSet<'_, S::Storage>,
    ) -> VisibleSlotNumber {
        self.chain_state.next_visible_rollup_height(state)
    }
}

impl<'b, S: Spec> BlobSelector for SoftConfirmationsKernel<'b, S> {
    type Spec = S;
    type BlobType = BlobDataWithId;
    const ACCEPTS_PREFERRED_BATCHES: bool = true;

    fn get_blobs_for_this_slot<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelStateAccessor<'k, <Self::Spec as Spec>::Storage>,
    ) -> anyhow::Result<BlobSelectorOutput<S, BlobDataWithId<IterableBatchWithId>>>
    where
        I: IntoIterator<Item = BlobOrigin<'a, <S::Da as DaSpec>::BlobTransaction>>,
    {
        self.blob_storage
            .get_blobs_for_this_slot(current_blobs, state)
    }
}

impl<'a, S: Spec> sov_modules_api::capabilities::ChainState for SoftConfirmationsKernel<'a, S> {
    type Spec = S;

    fn synchronise_chain(
        &self,
        slot_header: &<S::Da as DaSpec>::BlockHeader,
        validity_condition: &<S::Da as DaSpec>::ValidityCondition,
        pre_state_root: &<S::Storage as Storage>::Root,
        state: &mut sov_modules_api::KernelStateAccessor<S::Storage>,
    ) {
        self.chain_state
            .synchronize_chain(slot_header, validity_condition, pre_state_root, state);
    }

    fn finalise_chain_state(
        &self,
        gas_used: &S::Gas,
        state: &mut sov_modules_api::KernelStateAccessor<S::Storage>,
    ) {
        self.chain_state.finalize_chain_state(gas_used, state);
    }

    fn base_fee_per_gas(
        &self,
        state: &mut impl VersionReader<Error = Infallible>,
    ) -> Option<<<S as Spec>::Gas as Gas>::Price> {
        self.chain_state.base_fee_per_gas(state).unwrap_infallible()
    }

    fn current_visible_hash(
        &self,
        state: &mut sov_modules_api::KernelStateAccessor<S::Storage>,
    ) -> Option<<<Self::Spec as Spec>::Storage as Storage>::Root> {
        self.chain_state.current_visible_hash(state)
    }
}

/// These methods are used in the tests to access the internal state of the kernel.
/// Normally these should not be used, because everything happens inside the stf.
#[cfg(feature = "test-utils")]
impl<'a, S: Spec> SoftConfirmationsKernel<'a, S> {
    /// Gets a reference to the kernel's ChainState module.
    pub fn get_chain_state(&self) -> &sov_chain_state::ChainState<S> {
        self.chain_state
    }

    /// Gets a reference to the kernel's BlobStorage module.
    pub fn get_blob_storage(&self) -> &sov_blob_storage::BlobStorage<S> {
        self.blob_storage
    }
}
