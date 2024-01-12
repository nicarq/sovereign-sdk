use sov_blob_storage::BlobStorage;
use sov_chain_state::ChainState;
use sov_modules_api::runtime::capabilities::KernelSlotHooks;
use sov_modules_api::{Context, DaSpec};

use crate::kernels::basic::BasicKernel;
use crate::{Runtime, StfBlueprint};

impl<C: Context, Da: DaSpec, Vm, RT: Runtime<C, Da>, K: KernelSlotHooks<C, Da>>
    StfBlueprint<C, Da, Vm, RT, K>
{
    /// Returns the underlying kernel.
    pub fn kernel(&self) -> &K {
        &self.kernel
    }

    /// Returns the underlying runtime.
    pub fn runtime(&self) -> &RT {
        &self.runtime
    }
}

impl<C: Context, Da: DaSpec> BasicKernel<C, Da> {
    /// Returns the underlying blob storage.
    pub fn blob_storage(&self) -> &BlobStorage<C, Da> {
        &self.blob_storage
    }

    /// Returns the underlying chain state.
    pub fn chain_state(&self) -> &ChainState<C, Da> {
        &self.chain_state
    }
}
