use std::convert::Infallible;

use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{KernelStateAccessor, Spec, StateReader};
use sov_state::Kernel;

use crate::{ChainState, TransitionHeight};

impl<S: Spec> ChainState<S> {
    /// Get the visible height of the next slot.
    /// Panics if the slot number is not set
    pub fn get_next_visible_slot_number<Accessor: StateReader<Kernel, Error = Infallible>>(
        &self,
        accessor: &mut Accessor,
    ) -> TransitionHeight {
        self.next_visible_slot_number
            .get(accessor)
            .unwrap_infallible()
            .expect("The visible slot number should always be set")
    }

    /// Get the true height of the current slot.
    /// Panics if the slot number is not set
    pub fn get_true_rollup_height(
        &self,
        kernel_working_set: &mut KernelStateAccessor<S::Storage>,
    ) -> TransitionHeight {
        self.true_rollup_height(kernel_working_set)
            .unwrap_infallible()
    }
}
