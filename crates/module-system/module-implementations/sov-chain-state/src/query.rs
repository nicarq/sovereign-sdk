use std::convert::Infallible;

use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{KernelStateAccessor, Spec, StateReader, VisibleSlotNumber};
use sov_rollup_interface::common::SlotNumber;
use sov_state::Kernel;

use crate::ChainState;

impl<S: Spec> ChainState<S> {
    /// Get the visible height of the next slot.
    /// Panics if the rollup height is not set
    pub fn get_next_visible_rollup_height<Accessor: StateReader<Kernel, Error = Infallible>>(
        &self,
        accessor: &mut Accessor,
    ) -> VisibleSlotNumber {
        self.next_visible_rollup_height
            .get(accessor)
            .unwrap_infallible()
            .expect("The visible rollup height should always be set")
    }

    /// Get the true height of the current slot.
    /// Panics if the rollup height is not set
    pub fn get_true_rollup_height(
        &self,
        kernel_working_set: &mut KernelStateAccessor<S>,
    ) -> SlotNumber {
        self.true_rollup_height(kernel_working_set)
            .unwrap_infallible()
    }
}
