use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{
    DaSpec, GasSpec, KernelStateAccessor, Spec, StateAccessor, StateReader, VersionReader,
};
use sov_state::User;

use crate::ChainState;

impl<S, Da> ChainState<S, Da>
where
    S: Spec,
    Da: DaSpec,
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

    /// Returns the base fee per gas accessible at the current *virtual* slot.
    /// This value is safe to be used in the transaction execution context.
    ///
    /// ## Note
    /// If there is no in-progress transition at the current virtual slot, the initial base fee per gas is returned.
    pub fn base_fee_per_gas<Reader: VersionReader + StateAccessor>(
        &self,
        state: &mut Reader,
    ) -> Result<<S::Gas as sov_modules_api::Gas>::Price, <Reader as StateReader<User>>::Error> {
        if let Some(in_progress_transition) =
            self.slots.get(&(state.rollup_height_to_access()), state)?
        {
            let computed_base_fee =
                Self::compute_base_fee_per_gas(&in_progress_transition.gas_info);
            Ok(computed_base_fee)
        } else {
            Ok(<S as GasSpec>::initial_base_fee_per_gas())
        }
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
