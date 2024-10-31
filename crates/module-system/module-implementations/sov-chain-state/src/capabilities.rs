#[cfg(feature = "native")]
use sov_modules_api::capabilities::KernelWithSlotMapping;
use sov_modules_api::da::BlockHeaderTrait;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{DaSpec, GasSpec, KernelStateAccessor, KernelWriter, Spec, StateReader};
use sov_state::{Kernel, StateRoot, Storage};

use crate::{BlockGasInfo, ChainState, SlotInformation, VersionReader};

impl<S: Spec> ChainState<S> {
    /// Computes the current root hash available at the current *virtual* slot number.
    /// This is the kernel root hash at the *virtual* rollup height with the user root hash at the current height.
    /// Pratically, it merges the user root hash from the pre-state root with the kernel root hash at the specified height.
    ///
    /// ## Note
    /// If the state root at the current height is not available yet, this method will return `None`.
    pub fn current_visible_hash(
        &self,
        state: &mut KernelStateAccessor<'_, S::Storage>,
    ) -> Option<<S::Storage as Storage>::Root> {
        let current_root = self.state_roots.last(state).unwrap_infallible()?;

        let user_root = current_root.namespace_root(sov_state::ProvableNamespace::User);

        let root_at_height = self
            .root_at_height(state.visible_rollup_height(), state)
            .unwrap_infallible()?;

        let kernel_root = root_at_height.namespace_root(sov_state::ProvableNamespace::Kernel);

        Some(<S::Storage as Storage>::Root::from_namespace_roots(
            user_root,
            kernel_root,
        ))
    }

    /// Update the chain state at the beginning of the slot. Compute the next gas price
    pub fn synchronize_chain(
        &self,
        slot_header: &<<S as Spec>::Da as DaSpec>::BlockHeader,
        validity_condition: &<<S as Spec>::Da as DaSpec>::ValidityCondition,
        pre_state_root: &<S::Storage as Storage>::Root,
        state: &mut KernelStateAccessor<S::Storage>,
    ) {
        // We increment the rollup height at the very beginning of the slot execution
        self.increment_true_rollup_height(state);

        // The previous state root is set at the beginning of the next slot execution
        self.state_roots.push(pre_state_root, state);

        // There may not be a previous slot if the slot comes right after the genesis block
        let maybe_previous_slot = self.slots.last_entry_from_previous_slot(state);

        // We compute the base fee per gas from the previous slot if it exists
        let base_fee_per_gas = maybe_previous_slot
            .map(|previous_slot| Self::compute_base_fee_per_gas(&previous_slot.gas_info))
            .unwrap_or_else(|| S::initial_base_fee_per_gas());

        let gas_info = BlockGasInfo::new(
            // TODO(@theochap): the gas limit should be updated dynamically `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/271`
            S::initial_gas_limit(),
            base_fee_per_gas,
        );

        self.slots.push(
            &SlotInformation {
                hash: slot_header.hash(),
                validity_condition: *validity_condition,
                gas_info,
            },
            state,
        );

        self.time.set_true_current(&slot_header.time(), state);
    }

    /// Updates the gas used by the transition in progress at the end of each slot
    pub fn finalize_chain_state(
        &self,
        gas_used: &S::Gas,
        state: &mut KernelStateAccessor<S::Storage>,
    ) {
        // We retrieve the last slot in progress, update its gas information and store it back to the state
        let mut in_progress_slot = self
            .last_slot(state)
            .unwrap_infallible()
            .expect("There should always be a transition in progress");

        in_progress_slot.gas_info.update_gas_used(gas_used.clone());

        self.slots
            .set_last(&in_progress_slot, state)
            .expect("An error occurred while setting the last slot in progress. This is a bug. Please report it.");

        self.true_to_visible_rollup_height_history
            .set(
                &state.true_rollup_height(),
                &state.visible_rollup_height(),
                state,
            )
            .unwrap_infallible();
    }

    /// Returns the base fee per gas accessible at the specified slot height for this state accessor.
    pub fn base_fee_per_gas_at<Reader: VersionReader>(
        &self,
        height: u64,
        state: &mut Reader,
    ) -> Result<
        Option<<S::Gas as sov_modules_api::Gas>::Price>,
        <Reader as StateReader<Kernel>>::Error,
    > {
        if height == 0 {
            return Ok(Some(S::initial_base_fee_per_gas()));
        }

        Ok(
            if let Some(in_progress_transition) = self.slots.get(height, state)? {
                Some(in_progress_transition.gas_info.base_fee_per_gas)
            } else {
                None
            },
        )
    }

    /// Returns the base fee per gas accessible at the current *virtual* slot.
    /// This value is safe to be used in the transaction execution context.
    ///
    /// ## Note
    /// This method can return `None` if the base fee per gas for the current slot cannot be determined yet.
    /// This can happen when querying a slot too far ahead in the future.
    pub fn base_fee_per_gas<Reader: VersionReader>(
        &self,
        state: &mut Reader,
    ) -> Result<
        Option<<S::Gas as sov_modules_api::Gas>::Price>,
        <Reader as StateReader<Kernel>>::Error,
    > {
        self.base_fee_per_gas_at(state.rollup_height_to_access(), state)
    }
}

#[cfg(feature = "native")]
impl<S: Spec> KernelWithSlotMapping<S> for ChainState<S> {
    fn visible_rollup_height_at(
        &self,
        true_rollup_height: u64,
        state: &mut sov_modules_api::state::ApiStateAccessor<S>,
    ) -> Option<u64> {
        self.visible_rollup_height_at(true_rollup_height, state)
            .unwrap_infallible()
    }

    fn base_fee_per_gas_at(
        &self,
        height: u64,
        state: &mut sov_modules_api::state::ApiStateAccessor<S>,
    ) -> Option<<<S as Spec>::Gas as sov_modules_api::Gas>::Price> {
        self.base_fee_per_gas_at(height, state).unwrap_infallible()
    }
}
