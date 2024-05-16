//! The basic kernel provides censorship resistance by processing all blobs immediately in the order they appear on DA
use std::path::PathBuf;

use sov_blob_storage::BlobStorage;
use sov_chain_state::ChainState;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::kernel_state::BootstrapWorkingSet;
use sov_modules_api::runtime::capabilities::{BatchSelector, Kernel, KernelSlotHooks};
use sov_modules_api::{DaSpec, Gas, KernelModule, KernelWorkingSet, Spec};
use sov_state::Storage;

/// A kernel supporting based sequencing with soft confirmations
pub struct SoftConfirmationsKernel<S: Spec, Da: DaSpec> {
    phantom: std::marker::PhantomData<S>,
    chain_state: ChainState<S, Da>,
    blob_storage: BlobStorage<S, Da>,
}

impl<S: Spec, Da: DaSpec> Default for SoftConfirmationsKernel<S, Da> {
    fn default() -> Self {
        Self {
            phantom: std::marker::PhantomData,
            chain_state: Default::default(),
            blob_storage: Default::default(),
        }
    }
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

impl<S: Spec, Da: DaSpec> Kernel<S, Da> for SoftConfirmationsKernel<S, Da> {
    fn true_slot_number(&self, working_set: &mut BootstrapWorkingSet<'_, S>) -> u64 {
        self.chain_state.true_slot_number(working_set)
    }
    fn visible_slot_number(&self, working_set: &mut BootstrapWorkingSet<'_, S>) -> u64 {
        self.chain_state.next_visible_slot_number(working_set)
    }

    type GenesisConfig = SoftConfirmationsKernelGenesisConfig<S, Da>;

    #[cfg(feature = "native")]
    type GenesisPaths = SoftConfirmationsKernelGenesisPaths;

    fn genesis(
        &self,
        config: &Self::GenesisConfig,
        working_set: &mut KernelWorkingSet<'_, S>,
    ) -> Result<(), anyhow::Error> {
        Ok(self
            .chain_state
            .genesis_unchecked(&config.chain_state, working_set)?)
    }
}

impl<S: Spec, Da: DaSpec> BatchSelector<Da> for SoftConfirmationsKernel<S, Da> {
    type Spec = S;
    type Batch = BatchWithId;

    fn get_batches_for_this_slot<'a, 'k, I>(
        &self,
        current_blobs: I,
        _working_set: &mut sov_modules_api::KernelWorkingSet<'k, Self::Spec>,
    ) -> anyhow::Result<Vec<(Self::Batch, Da::Address)>>
    where
        I: IntoIterator<Item = &'a mut Da::BlobTransaction>,
    {
        self.blob_storage
            .get_batches_for_this_slot(current_blobs, _working_set)
    }
}

impl<S: Spec, Da: DaSpec> KernelSlotHooks<S, Da> for SoftConfirmationsKernel<S, Da> {
    fn begin_slot_hook(
        &self,
        slot_header: &<Da as DaSpec>::BlockHeader,
        validity_condition: &<Da as DaSpec>::ValidityCondition,
        pre_state_root: &<<Self::Spec as sov_modules_api::Spec>::Storage as Storage>::Root,
        state_checkpoint: &mut sov_modules_api::StateCheckpoint<Self::Spec>,
    ) -> <S::Gas as Gas>::Price {
        let mut ws = sov_modules_api::KernelWorkingSet::from_kernel(self, state_checkpoint);
        self.chain_state
            .begin_slot_hook(slot_header, validity_condition, pre_state_root, &mut ws)
    }

    fn end_slot_hook(
        &self,
        gas_used: &S::Gas,
        state_checkpoint: &mut sov_modules_api::StateCheckpoint<Self::Spec>,
    ) {
        let mut ws = sov_modules_api::KernelWorkingSet::from_kernel(self, state_checkpoint);
        self.chain_state.end_slot_hook(gas_used, &mut ws);
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
