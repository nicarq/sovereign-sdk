use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{DaSpec, KernelWorkingSet, Spec};

use crate::ChainState;

impl<S, Da> ChainState<S, Da>
where
    S: Spec,
    Da: DaSpec,
{
    /// Increment the current slot number
    /// This function also modifies the kernel working set to update the true height.
    pub(crate) fn increment_true_slot_number(&self, state: &mut KernelWorkingSet<S>) {
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

    /// Returns the base fee per gas accessible at the current *virtual* slot.
    /// This value is safe to be used in the transaction execution context.
    ///
    /// ## Note
    /// If there is no in-progress transition at the current virtual slot, the initial base fee per gas is returned.
    pub fn base_fee_per_gas(
        &self,
        state: &mut KernelWorkingSet<S>,
    ) -> <S::Gas as sov_modules_api::Gas>::Price {
        if let Some(in_progress_transition) = self.in_progress_transition.get(
            &self.next_visible_slot_number(state).unwrap_infallible(),
            state,
        ) {
            in_progress_transition.gas_info.base_fee_per_gas
        } else {
            Self::initial_base_fee_per_gas()
        }
    }
}
