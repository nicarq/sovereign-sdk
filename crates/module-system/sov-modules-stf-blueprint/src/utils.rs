use sov_modules_api::runtime::capabilities::KernelSlotHooks;
use sov_modules_api::Spec;

use crate::{Runtime, StfBlueprint};

impl<S: Spec, RT: Runtime<S>, K: KernelSlotHooks<S>> StfBlueprint<S, RT, K> {
    /// Returns the underlying kernel.
    pub fn kernel(&self) -> &K {
        &self.kernel
    }

    /// Returns the underlying runtime.
    pub fn runtime(&self) -> &RT {
        &self.runtime
    }
}
