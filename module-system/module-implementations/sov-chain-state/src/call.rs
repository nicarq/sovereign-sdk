use sov_modules_api::prelude::*;
use sov_modules_api::{Context, DaSpec, WorkingSet};
use sov_state::storage::KernelWorkingSet;

use crate::{ChainState, StateTransitionId, TransitionHeight};

impl<C, Da> ChainState<C, Da>
where
    C: Context,
    Da: DaSpec,
{
    /// Increment the current slot number
    /// This function also modifies the kernel working set to update the true height.
    pub(crate) fn increment_true_slot_number(&self, working_set: &mut KernelWorkingSet<C>) {
        let current_height = self.true_slot_number.get(working_set).unwrap_or_default();
        let new_height = current_height.saturating_add(1);
        self.true_slot_number.set(&(new_height), working_set);

        working_set.update_true_slot_number(new_height);
    }

    /// Store the previous state transition
    pub(crate) fn store_state_transition(
        &self,
        height: TransitionHeight,
        transition: StateTransitionId<C, Da>,
        working_set: &mut WorkingSet<C>,
    ) {
        self.historical_transitions
            .set(&height, &transition, working_set);
    }
}
