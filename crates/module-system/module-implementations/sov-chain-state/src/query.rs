use std::convert::Infallible;

use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{KernelStateAccessor, Spec, StateReader};
use sov_rollup_interface::common::{SlotNumber, VisibleSlotNumber};
use sov_state::Kernel;

use crate::ChainState;

impl<S: Spec> ChainState<S> {
    /// Get the visible height of the next slot.
    /// Panics if the rollup height is not set
    pub fn get_next_visible_slot_number<Accessor: StateReader<Kernel, Error = Infallible>>(
        &self,
        accessor: &mut Accessor,
    ) -> VisibleSlotNumber {
        self.next_visible_slot_number
            .get(accessor)
            .unwrap_infallible()
            .expect("The visible rollup height should always be set")
    }

    /// Get the true height of the current slot.
    /// Panics if the rollup height is not set
    pub fn get_true_slot_number(
        &self,
        kernel_working_set: &mut KernelStateAccessor<S>,
    ) -> SlotNumber {
        self.true_slot_number(kernel_working_set)
            .unwrap_infallible()
    }
}
