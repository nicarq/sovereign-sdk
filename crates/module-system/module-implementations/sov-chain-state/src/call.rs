use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{KernelStateAccessor, Spec};

use crate::ChainState;

impl<S> ChainState<S>
where
    S: Spec,
{
    /// Increment the current rollup height
    /// This function also modifies the kernel working set to update the true height.
    pub(crate) fn increment_true_rollup_height(&self, state: &mut KernelStateAccessor<S>) {
        let current_height = self
            .true_rollup_height
            .get(state)
            .unwrap_infallible()
            .unwrap_or_default();
        let new_height = current_height.saturating_add(1);

        self.true_rollup_height
            .set(&(new_height), state)
            .unwrap_infallible();

        state.update_true_rollup_height(new_height);
    }
}
