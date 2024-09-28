//! The basic kernel provides censorship resistance by processing all blobs immediately in the order they appear on DA
use std::path::PathBuf;

use sov_blob_storage::BlobStorage;
use sov_chain_state::ChainState;
use sov_modules_api::capabilities::BlobOrigin;
#[cfg(feature = "native")]
use sov_modules_api::capabilities::KernelWithSlotMapping;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::runtime::capabilities::{BlobSelector, Kernel, KernelSlotHooks};
use sov_modules_api::{
    BlobDataWithId, BootstrapWorkingSet, DaSpec, Gas, KernelModule, KernelStateAccessor, Spec,
};
use sov_state::Storage;

/// A kernel supporting based sequencing with soft confirmations
#[derive(Default)]
pub struct SoftConfirmationsKernel<S: Spec, Da: DaSpec> {
    chain_state: ChainState<S, Da>,
    blob_storage: BlobStorage<S, Da>,
}

/// Path information required to initialize a basic kernel from files
pub struct SoftConfirmationsKernelGenesisPaths {
    /// The path to the chain_state genesis config
    pub chain_state: PathBuf,
}

pub struct SoftConfirmationsKernelGenesisConfig<S: Spec, Da: DaSpec> {
    /// The chain state genesis config
    pub chain_state: <ChainState<S, Da> as KernelModule>::Config,
}

#[cfg(feature = "native")]
impl<S: Spec, Da: DaSpec> KernelWithSlotMapping<S> for SoftConfirmationsKernel<S, Da> {
    fn visible_slot_number_at(
        &self,
        true_slot_number: u64,
        state: &mut sov_modules_api::ApiStateAccessor<S>,
    ) -> u64 {
        self.chain_state
            .visible_slot_number_at(true_slot_number, state)
            .unwrap_infallible()
    }
}

impl<S: Spec, Da: DaSpec> Kernel<S::Storage> for SoftConfirmationsKernel<S, Da> {
    type GenesisConfig = SoftConfirmationsKernelGenesisConfig<S, Da>;

    #[cfg(feature = "native")]
    type GenesisPaths = SoftConfirmationsKernelGenesisPaths;

    fn true_slot_number(&self, state: &mut BootstrapWorkingSet<'_, S::Storage>) -> u64 {
        self.chain_state.true_slot_number(state).unwrap_infallible()
    }
    fn visible_slot_number(&self, state: &mut BootstrapWorkingSet<'_, S::Storage>) -> u64 {
        self.chain_state.next_visible_slot_number(state)
    }

    fn genesis(
        &self,
        config: &Self::GenesisConfig,
        state: &mut KernelStateAccessor<S::Storage>,
    ) -> anyhow::Result<()> {
        Ok(self
            .chain_state
            .genesis_unchecked(&config.chain_state, state)?)
    }
}

impl<S: Spec, Da: DaSpec> BlobSelector<Da> for SoftConfirmationsKernel<S, Da> {
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
        self.blob_storage
            .get_blobs_for_this_slot(current_blobs, state)
    }
}

impl<S: Spec, Da: DaSpec> KernelSlotHooks<S, Da> for SoftConfirmationsKernel<S, Da> {
    fn begin_slot_hook(
        &self,
        slot_header: &<Da as DaSpec>::BlockHeader,
        validity_condition: &<Da as DaSpec>::ValidityCondition,
        pre_state_root: &<<Self::Spec as sov_modules_api::Spec>::Storage as Storage>::Root,
        state: &mut sov_modules_api::KernelStateAccessor<<Self::Spec as Spec>::Storage>,
    ) -> <S::Storage as Storage>::Root {
        self.chain_state
            .begin_slot_hook(slot_header, validity_condition, pre_state_root, state)
    }

    fn end_slot_hook(
        &self,
        gas_used: &S::Gas,
        state: &mut sov_modules_api::KernelStateAccessor<<Self::Spec as Spec>::Storage>,
    ) -> Option<[u8; 32]> {
        self.chain_state.end_slot_hook(gas_used, state)
    }

    fn base_fee_per_gas(
        &self,
        state: &mut sov_modules_api::StateCheckpoint<<Self::Spec as Spec>::Storage>,
    ) -> <<S as Spec>::Gas as Gas>::Price {
        self.chain_state.base_fee_per_gas(state).unwrap_infallible()
    }
}

/// These methods are used in the tests to access the internal state of the kernel.
/// Normally these should not be used, because everything happens inside the stf.
#[cfg(feature = "test-utils")]
impl<S: Spec, Da: DaSpec> SoftConfirmationsKernel<S, Da> {
    /// Gets a reference to the kernel's ChainState module.
    pub fn get_chain_state(&self) -> &sov_chain_state::ChainState<S, Da> {
        &self.chain_state
    }

    /// Gets a reference to the kernel's BlobStorage module.
    pub fn get_blob_storage(&self) -> &sov_blob_storage::BlobStorage<S, Da> {
        &self.blob_storage
    }
}
