use sov_modules_api::{DaSpec, Spec};
use sov_state::storage::KernelWorkingSet;

use crate::ChainState;

impl<S, Da> ChainState<S, Da>
where
    S: Spec,
    Da: DaSpec,
{
    /// Increment the current slot number
    /// This function also modifies the kernel working set to update the true height.
    pub(crate) fn increment_true_slot_number(&self, working_set: &mut KernelWorkingSet<S>) {
        let current_height = self.true_slot_number.get(working_set).unwrap_or_default();
        let new_height = current_height.saturating_add(1);
        self.true_slot_number.set(&(new_height), working_set);

        working_set.update_true_slot_number(new_height);
    }
}
