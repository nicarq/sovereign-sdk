use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{KernelStateAccessor, Spec};

use crate::ChainState;

impl<S> ChainState<S>
where
    S: Spec,
{
    /// Increment the current slot number
    /// This function also modifies the kernel working set to update the true height.
    pub(crate) fn increment_true_slot_number(&self, state: &mut KernelStateAccessor<S::Storage>) {
        let current_height = self
            .true_slot_number
            .get(state)
            .unwrap_infallible()
            .unwrap_or_default();
        let new_height = current_height.saturating_add(1);

        self.true_slot_number
            .set(&(new_height), state)
            .unwrap_infallible();

        state.update_true_slot_number(new_height);
    }
}
