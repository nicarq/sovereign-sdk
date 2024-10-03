use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{DaSpec, KernelStateAccessor, Spec};

use crate::{ChainState, TransitionHeight};

impl<S: Spec, Da: DaSpec> ChainState<S, Da> {
    /// Get the visible height of the next slot.
    /// Panics if the slot number is not set
    pub fn get_next_visible_slot_number(
        &self,
        kernel_working_set: &mut KernelStateAccessor<S::Storage>,
    ) -> TransitionHeight {
        self.next_visible_slot_number
            .get(kernel_working_set)
            .unwrap_infallible()
            .expect("The visible slot number should always be set")
    }

    /// Get the true height of the current slot.
    /// Panics if the slot number is not set
    pub fn get_true_slot_number(
        &self,
        kernel_working_set: &mut KernelStateAccessor<S::Storage>,
    ) -> TransitionHeight {
        self.true_slot_number(kernel_working_set)
            .unwrap_infallible()
    }
}
