//! The basic kernel provides censorship resistance by processing all blobs immediately in the order they appear on DA

use sov_blob_storage::BlobStorage;
use sov_chain_state::ChainState;
use sov_modules_api::capabilities::BlobOrigin;
#[cfg(feature = "native")]
use sov_modules_api::capabilities::KernelWithSlotMapping;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::runtime::capabilities::{BlobSelector, Kernel, KernelSlotHooks};
use sov_modules_api::{
    BlobDataWithId, BootstrapWorkingSet, DaSpec, Gas, KernelStateAccessor, Spec,
};
use sov_state::Storage;

/// The simplest imaginable kernel. It does not do any batching or reordering of blobs.
#[derive(Clone)]
pub struct BasicKernel<S: Spec, Da: DaSpec> {
    pub(crate) chain_state: ChainState<S, Da>,
    pub(crate) blob_storage: BlobStorage<S, Da>,
}

impl<S: Spec, Da: DaSpec> BasicKernel<S, Da> {
    /// Gets a reference to the kernel's ChainState module.
    pub fn chain_state(&self) -> &ChainState<S, Da> {
        &self.chain_state
    }

    /// Gets a reference to the kernel's BlobStorage module.
    pub fn blob_storage(&self) -> &BlobStorage<S, Da> {
        &self.blob_storage
    }
}

impl<S: Spec, Da: DaSpec> Default for BasicKernel<S, Da> {
    fn default() -> Self {
        Self {
            chain_state: Default::default(),
            blob_storage: Default::default(),
        }
    }
}

impl<S: Spec, Da: DaSpec> Kernel<S> for BasicKernel<S, Da> {
    fn true_slot_number(&self, state: &mut BootstrapWorkingSet<'_, S::Storage>) -> u64 {
        self.chain_state.true_slot_number(state).unwrap_infallible()
    }

    fn next_visible_slot_number(&self, state: &mut BootstrapWorkingSet<'_, S::Storage>) -> u64 {
        self.chain_state.true_slot_number(state).unwrap_infallible()
    }
}

impl<S: Spec, Da: DaSpec> BlobSelector<Da> for BasicKernel<S, Da> {
    type Spec = S;

    type BlobType = BlobDataWithId;

    fn get_blobs_for_this_slot<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelStateAccessor<'k, <Self::Spec as Spec>::Storage>,
    ) -> anyhow::Result<Vec<(Self::BlobType, Da::Address)>>
    where
        I: IntoIterator<Item = BlobOrigin<'a, Da::BlobTransaction>>,
    {
        Ok(self
            .blob_storage
            .select_blobs_as_based_sequencer(current_blobs, state))
    }
}

#[cfg(feature = "native")]
impl<S: Spec, Da: DaSpec> KernelWithSlotMapping<S> for BasicKernel<S, Da> {
    fn visible_slot_number_at(
        &self,
        true_slot_number: u64,
        _state: &mut sov_modules_api::ApiStateAccessor<S>,
    ) -> u64 {
        true_slot_number
    }
}

impl<S: Spec, Da: DaSpec> KernelSlotHooks<S, Da> for BasicKernel<S, Da> {
    fn begin_slot_hook(
        &self,
        slot_header: &<Da as DaSpec>::BlockHeader,
        validity_condition: &<Da as DaSpec>::ValidityCondition,
        pre_state_root: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        state: &mut sov_modules_api::KernelStateAccessor<<Self::Spec as Spec>::Storage>,
    ) -> <S::Storage as Storage>::Root {
        self.chain_state
            .begin_slot_hook(slot_header, validity_condition, pre_state_root, state)
    }

    fn end_slot_hook(
        &self,
        gas_used: &S::Gas,
        state: &mut sov_modules_api::KernelStateAccessor<<Self::Spec as Spec>::Storage>,
    ) {
        self.chain_state.end_slot_hook(gas_used, state);
    }

    fn base_fee_per_gas(
        &self,
        state: &mut sov_modules_api::StateCheckpoint<<Self::Spec as Spec>::Storage>,
    ) -> <<S as Spec>::Gas as Gas>::Price {
        self.chain_state.base_fee_per_gas(state).unwrap_infallible()
    }
}
