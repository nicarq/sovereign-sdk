use jsonrpsee::core::RpcResult;
// use jsonrpsee::core::RpcResult;
use sov_modules_api::KernelWorkingSet;

// use sov_modules_api::WorkingSet;
use crate::{ChainState, TransitionHeight};

// TODO: Implement RPC methods compatible with Kernel State
// #[rpc_gen(client, server, namespace = "chainState")]
impl<C: sov_modules_api::Context, Da: sov_modules_api::DaSpec> ChainState<C, Da> {
    /// Get the visible height of the next slot.
    /// Panics if the slot number is not set
    // #[rpc_method(name = "getTrueSlotNumber")]
    pub fn get_next_visible_slot_number(
        &self,
        kernel_working_set: &mut KernelWorkingSet<C>,
    ) -> RpcResult<TransitionHeight> {
        Ok(self.next_visible_slot_number(kernel_working_set))
    }

    /// Get the true height of the current slot.
    /// Panics if the slot number is not set
    // #[rpc_method(name = "getVisibleSlotNumber")]
    pub fn get_true_slot_number(
        &self,
        kernel_working_set: &mut KernelWorkingSet<C>,
    ) -> RpcResult<TransitionHeight> {
        Ok(self.true_slot_number(kernel_working_set))
    }
}
