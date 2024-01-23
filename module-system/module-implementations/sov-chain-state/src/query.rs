use jsonrpsee::core::RpcResult;
// use jsonrpsee::core::RpcResult;
use sov_modules_api::KernelWorkingSet;

// use sov_modules_api::WorkingSet;
use crate::{ChainState, TransitionHeight};

// TODO: Implement RPC methods compatible with Kernel State
// #[rpc_gen(client, server, namespace = "chainState")]
impl<C: sov_modules_api::Context, Da: sov_modules_api::DaSpec> ChainState<C, Da> {
    /// Get the true height of the current slot.
    /// Panics if the slot height is not set
    // #[rpc_method(name = "getTrueSlotHeight")]
    pub fn get_visible_slot_height(
        &self,
        kernel_working_set: &mut KernelWorkingSet<C>,
    ) -> RpcResult<TransitionHeight> {
        Ok(self.visible_slot_height(kernel_working_set))
    }

    /// Get the visible height of the current slot.
    /// Panics if the slot height is not set
    // #[rpc_method(name = "getVisibleSlotHeight")]
    pub fn get_true_slot_height(
        &self,
        kernel_working_set: &mut KernelWorkingSet<C>,
    ) -> RpcResult<TransitionHeight> {
        Ok(self.true_slot_height(kernel_working_set))
    }
}
