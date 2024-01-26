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
    /// Gets a reference to the kernel's ChainState module.
    pub fn chain_state(&self) -> &sov_chain_state::ChainState<C, Da> {
        &self.chain_state
    }

    /// Gets a reference to the kernel's BlobStorage module.
    pub fn blob_storage(&self) -> &sov_blob_storage::BlobStorage<C, Da> {
        &self.blob_storage
    }
}
