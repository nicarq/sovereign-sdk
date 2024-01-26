//! The basic kernel provides censorship resistance by processing all blobs immediately in the order they appear on DA
use std::path::PathBuf;

use sov_blob_storage::BlobStorage;
use sov_chain_state::ChainState;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::runtime::capabilities::{BatchSelector, Kernel, KernelSlotHooks};
use sov_modules_api::{Context, DaSpec, KernelModule, KernelWorkingSet};
use sov_state::storage::kernel_state::BootstrapWorkingSet;
use sov_state::Storage;

/// The simplest imaginable kernel. It does not do any batching or reordering of blobs.
pub struct BasicKernel<C: Context, Da: DaSpec> {
    pub(crate) chain_state: ChainState<C, Da>,
    pub(crate) blob_storage: BlobStorage<C, Da>,
}

impl<C: Context, Da: DaSpec> Default for BasicKernel<C, Da> {
    fn default() -> Self {
        Self {
            chain_state: Default::default(),
            blob_storage: Default::default(),
        }
    }
}

/// Path information required to initialize a basic kernel from files
pub struct BasicKernelGenesisPaths {
    /// The path to the chain_state genesis config
    pub chain_state: PathBuf,
}

/// The genesis configuration for the basic kernel
pub struct BasicKernelGenesisConfig<C: Context, Da: DaSpec> {
    /// The chain state genesis config
    pub chain_state: <ChainState<C, Da> as KernelModule>::Config,
}

impl<C: Context, Da: DaSpec> Kernel<C, Da> for BasicKernel<C, Da> {
    fn true_height(&self, working_set: &mut BootstrapWorkingSet<'_, C>) -> u64 {
        self.chain_state.true_slot_height(working_set)
    }
    fn visible_height(&self, working_set: &mut BootstrapWorkingSet<'_, C>) -> u64 {
        self.chain_state.true_slot_height(working_set)
    }

    type GenesisConfig = BasicKernelGenesisConfig<C, Da>;

    #[cfg(feature = "native")]
    type GenesisPaths = BasicKernelGenesisPaths;

    fn genesis(
        &self,
        config: &Self::GenesisConfig,
        working_set: &mut KernelWorkingSet<'_, C>,
    ) -> Result<(), anyhow::Error> {
        self.chain_state
            .genesis_unchecked(&config.chain_state, working_set)?;
        self.blob_storage.genesis_unchecked(&(), working_set)?;
        Ok(())
    }
}

impl<C: Context, Da: DaSpec> BatchSelector<Da> for BasicKernel<C, Da> {
    type Context = C;

    type Batch = BatchWithId;

    fn get_batches_for_this_slot<'a, 'k, I>(
        &self,
        current_blobs: I,
        working_set: &mut sov_modules_api::KernelWorkingSet<'k, Self::Context>,
    ) -> anyhow::Result<Vec<(Self::Batch, Da::Address)>>
    where
        I: IntoIterator<Item = &'a mut Da::BlobTransaction>,
    {
        Ok(self
            .blob_storage
            .select_blobs_as_based_sequencer(current_blobs, working_set))
    }
}

impl<C: Context, Da: DaSpec> KernelSlotHooks<C, Da> for BasicKernel<C, Da> {
    fn begin_slot_hook(
        &self,
        slot_header: &<Da as DaSpec>::BlockHeader,
        validity_condition: &<Da as DaSpec>::ValidityCondition,
        pre_state_root: &<<Self::Context as sov_modules_api::Spec>::Storage as Storage>::Root,
        working_set: &mut sov_modules_api::WorkingSet<Self::Context>,
    ) {
        let mut ws = sov_modules_api::KernelWorkingSet::from_kernel(self, working_set);
        self.chain_state
            .begin_slot_hook(slot_header, validity_condition, pre_state_root, &mut ws);
    }

    fn end_slot_hook(&self, working_set: &mut sov_modules_api::WorkingSet<Self::Context>) {
        let mut ws = sov_modules_api::KernelWorkingSet::from_kernel(self, working_set);
        self.chain_state.end_slot_hook(&mut ws);
    }
}
