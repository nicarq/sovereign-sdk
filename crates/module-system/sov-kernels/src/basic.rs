//! The basic kernel provides censorship resistance by processing all blobs immediately in the order they appear on DA

use std::convert::Infallible;

use sov_blob_storage::BlobStorage;
use sov_chain_state::ChainState;
use sov_modules_api::capabilities::{BlobOrigin, BlobSelectorOutput};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::runtime::capabilities::{BlobSelector, Kernel};
use sov_modules_api::{
    BlobDataWithId, BootstrapWorkingSet, DaSpec, Gas, KernelStateAccessor, Spec, VersionReader,
};
use sov_state::Storage;

/// The simplest imaginable kernel. It does not do any batching or reordering of blobs.
pub struct BasicKernel<'a, S: Spec> {
    pub chain_state: &'a ChainState<S>,
    pub blob_storage: &'a BlobStorage<S>,
}

impl<'a, S: Spec> BasicKernel<'a, S> {
    /// Gets a reference to the kernel's ChainState module.
    pub fn chain_state(&self) -> &ChainState<S> {
        self.chain_state
    }

    /// Gets a reference to the kernel's BlobStorage module.
    pub fn blob_storage(&self) -> &BlobStorage<S> {
        self.blob_storage
    }
}

impl<'a, S: Spec> Kernel<S> for BasicKernel<'a, S> {
    fn true_rollup_height(&self, state: &mut BootstrapWorkingSet<'_, S::Storage>) -> u64 {
        self.chain_state
            .true_rollup_height(state)
            .unwrap_infallible()
    }

    fn next_visible_rollup_height(&self, state: &mut BootstrapWorkingSet<'_, S::Storage>) -> u64 {
        self.chain_state
            .true_rollup_height(state)
            .unwrap_infallible()
    }
}

impl<'b, S: Spec> BlobSelector for BasicKernel<'b, S> {
    type Spec = S;

    type BlobType = BlobDataWithId;

    fn get_blobs_for_this_slot<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelStateAccessor<'k, <Self::Spec as Spec>::Storage>,
    ) -> anyhow::Result<BlobSelectorOutput<S, BlobDataWithId>>
    where
        I: IntoIterator<Item = BlobOrigin<'a, <S::Da as DaSpec>::BlobTransaction>>,
    {
        Ok(self
            .blob_storage
            .select_blobs_as_based_sequencer(current_blobs, state))
    }
}

impl<'a, S: Spec> sov_modules_api::capabilities::ChainState for BasicKernel<'a, S> {
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
