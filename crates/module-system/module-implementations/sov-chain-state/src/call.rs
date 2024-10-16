use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{GasSpec, KernelStateAccessor, Spec};

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

    /// Returns the *virtual* base fee per gas contained in a [`KernelStateAccessor`].
    ///
    /// TODO(@theochap, `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1479>`): remove once the linked issue is fixed. This should be unified with the `base_fee_per_gas` method above.
    pub fn virtual_base_fee_per_gas(
        &self,
        state: &mut KernelStateAccessor<S::Storage>,
    ) -> <S::Gas as sov_modules_api::Gas>::Price {
        if let Some(in_progress_transition) = self
            .slots
            .get(&(state.virtual_slot_number()), state)
            .unwrap_infallible()
        {
            Self::compute_base_fee_per_gas(&in_progress_transition.gas_info)
        } else {
            <S as GasSpec>::initial_base_fee_per_gas()
        }
    }
}
