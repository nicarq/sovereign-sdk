use sov_modules_api::runtime::capabilities::KernelSlotHooks;
use sov_modules_api::{DaSpec, Spec};

use crate::kernels::basic::BasicKernel;
use crate::{Runtime, StfBlueprint};

impl<S: Spec, Da: DaSpec, Vm, RT: Runtime<S, Da>, K: KernelSlotHooks<S, Da>>
    StfBlueprint<S, Da, Vm, RT, K>
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

impl<S: Spec, Da: DaSpec> BasicKernel<S, Da> {
    /// Gets a reference to the kernel's ChainState module.
    pub fn chain_state(&self) -> &sov_chain_state::ChainState<S, Da> {
        &self.chain_state
    }

    /// Gets a reference to the kernel's BlobStorage module.
    pub fn blob_storage(&self) -> &sov_blob_storage::BlobStorage<S, Da> {
        &self.blob_storage
    }
}
