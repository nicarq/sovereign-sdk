use sov_modules_api::prelude::*;
use sov_modules_api::{Context, DaSpec, WorkingSet};
use sov_state::storage::KernelWorkingSet;

use crate::{ChainState, StateTransitionId, TransitionHeight};

impl<C, Da> ChainState<C, Da>
where
    C: Context,
    Da: DaSpec,
{
    /// Increment the current slot height
    pub(crate) fn increment_true_slot_height(&self, working_set: &mut KernelWorkingSet<C>) {
        let current_height = self.true_height.get(working_set.inner).unwrap_or_default();
        self.true_height
            .set(&(current_height.saturating_add(1)), working_set.inner);
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
