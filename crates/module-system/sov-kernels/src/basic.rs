//! The basic kernel provides censorship resistance by processing all blobs immediately in the order they appear on DA

use std::convert::Infallible;

use sov_blob_storage::BlobStorage;
use sov_chain_state::ChainState;
use sov_modules_api::capabilities::{BlobOrigin, BlobSelectorOutput, BlockGasInfo, RollupHeight};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::runtime::capabilities::{BlobSelector, Kernel as KernelTrait};
use sov_modules_api::{
    BlobDataWithId, BootstrapWorkingSet, DaSpec, Gas, IterableBatchWithId, KernelStateAccessor,
    Spec, StateReader, VersionReader, VisibleSlotNumber,
};
use sov_rollup_interface::common::SlotNumber;
use sov_state::{Kernel, Storage, User};

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

impl<'a, S: Spec> KernelTrait<S> for BasicKernel<'a, S> {
    fn true_slot_number(&self, state: &mut BootstrapWorkingSet<'_, S>) -> SlotNumber {
        self.chain_state.true_slot_number(state).unwrap_infallible()
    }

    fn next_visible_slot_number(
        &self,
        state: &mut BootstrapWorkingSet<'_, S>,
    ) -> VisibleSlotNumber {
        self.chain_state.next_visible_slot_number(state)
    }

    fn rollup_height(&self, state: &mut BootstrapWorkingSet<'_, S>) -> RollupHeight {
        self.chain_state.rollup_height(state).unwrap_infallible()
    }

    fn record_gas_usage(
        &self,
        state: &mut sov_modules_api::StateCheckpoint<S>,
        gas_info: BlockGasInfo<S::Gas>,
        rollup_height: RollupHeight,
    ) {
        self.chain_state
            .record_gas_usage(state, gas_info, rollup_height);
    }
}

impl<'b, S: Spec> BlobSelector for BasicKernel<'b, S> {
    type Spec = S;

    type BlobType = BlobDataWithId;

    const ACCEPTS_PREFERRED_BATCHES: bool = false;

    fn get_blobs_for_this_slot<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelStateAccessor<'k, Self::Spec>,
    ) -> anyhow::Result<BlobSelectorOutput<S, BlobDataWithId<IterableBatchWithId>>>
    where
        I: IntoIterator<Item = BlobOrigin<'a, <S::Da as DaSpec>::BlobTransaction>>,
    {
        Ok(self
            .blob_storage
            .select_blobs_as_based_sequencer(current_blobs, state)
            .map_blobs(|b| b.map_batch(IterableBatchWithId::new)))
    }
}

impl<'a, S: Spec> sov_modules_api::capabilities::ChainState for BasicKernel<'a, S> {
    type Spec = S;

    fn synchronise_chain(
        &self,
        slot_header: &<S::Da as DaSpec>::BlockHeader,
        validity_condition: &<S::Da as DaSpec>::ValidityCondition,
        pre_state_root: &<S::Storage as Storage>::Root,
        state: &mut sov_modules_api::KernelStateAccessor<S>,
    ) {
        self.chain_state
            .synchronize_chain(slot_header, validity_condition, pre_state_root, state);
    }

    fn finalise_chain_state(
        &self,
        gas_used: &S::Gas,
        state: &mut sov_modules_api::KernelStateAccessor<S>,
    ) {
        self.chain_state.finalize_chain_state(gas_used, state);
    }

    fn base_fee_per_gas<
        Reader: VersionReader
            + StateReader<Kernel, Error = Infallible>
            + StateReader<User, Error = Infallible>,
    >(
        &self,
        state: &mut Reader,
    ) -> Option<<<S as Spec>::Gas as Gas>::Price> {
        self.chain_state.base_fee_per_gas(state).unwrap_infallible()
    }

    fn block_gas_limit<
        Reader: VersionReader
            + StateReader<Kernel, Error = Infallible>
            + StateReader<User, Error = Infallible>,
    >(
        &self,
        state: &mut Reader,
    ) -> Option<<Self::Spec as Spec>::Gas> {
        self.chain_state.block_gas_limit(state).unwrap_infallible()
    }

    fn current_visible_hash(
        &self,
        state: &mut sov_modules_api::KernelStateAccessor<S>,
    ) -> Option<<<Self::Spec as Spec>::Storage as Storage>::Root> {
        self.chain_state.current_visible_hash(state)
    }

    fn increment_rollup_height(
        &self,
        state: &mut KernelStateAccessor<'_, Self::Spec>,
        visible_slot_number: VisibleSlotNumber,
    ) {
        self.chain_state
            .increment_rollup_height(state, visible_slot_number);
    }
}
