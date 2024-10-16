#[cfg(feature = "native")]
use sov_modules_api::capabilities::KernelWithSlotMapping;
use sov_modules_api::da::BlockHeaderTrait;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{DaSpec, GasSpec, KernelStateAccessor, KernelWriter, Spec};
use sov_state::{StateRoot, Storage};

use crate::{BlockGasInfo, ChainState, SlotInformation};

impl<S: Spec> ChainState<S> {
    /// Computes the current root hash available at the current *virtual* slot number.
    /// This is the kernel root hash at the *virtual* rollup height with the user root hash at the current height.
    fn current_visible_hash(
        &self,
        pre_state_root: &<S::Storage as Storage>::Root,
        state: &mut KernelStateAccessor<S::Storage>,
    ) -> <S::Storage as Storage>::Root {
        let user_root = pre_state_root.namespace_root(sov_state::ProvableNamespace::User);

        let kernel_root = if let Some(root) = self
            .get_root_at_height(state.virtual_slot_number(), state)
            .unwrap_infallible()
        {
            root.clone()
        } else {
            self.get_genesis_hash(state)
                .unwrap_infallible()
                .expect("Genesis height should always be set.")
        }
        .namespace_root(sov_state::ProvableNamespace::Kernel);

        <S::Storage as Storage>::Root::from_namespace_roots(user_root, kernel_root)
    }

    /// Update the chain state at the beginning of the slot. Compute the next gas price
    pub fn begin_slot_hook(
        &self,
        slot_header: &<<S as Spec>::Da as DaSpec>::BlockHeader,
        validity_condition: &<<S as Spec>::Da as DaSpec>::ValidityCondition,
        pre_state_root: &<S::Storage as Storage>::Root,
        state: &mut KernelStateAccessor<S::Storage>,
    ) -> <S::Storage as Storage>::Root {
        // We increment the slot number at the very beginning of the slot execution
        self.increment_true_slot_number(state);

        let slot_number = state.true_slot_number();

        let previous_slot_number = slot_number
            .checked_sub(1)
            .expect("The slot number should be strictly greater than zero!");

        // The previous state root is set at the beginning of the next slot execution
        self.state_roots
            .set(&(previous_slot_number), pre_state_root, state)
            .unwrap_infallible();

        // There may not be a previous slot if the slot comes right after the genesis block
        let maybe_previous_slot = self
            .slots
            .get(&(previous_slot_number), state)
            .unwrap_infallible();

        // We compute the base fee per gas from the previous slot if it exists
        let base_fee_per_gas = maybe_previous_slot
            .map(|previous_slot| Self::compute_base_fee_per_gas(&previous_slot.gas_info))
            .unwrap_or_else(|| S::initial_base_fee_per_gas());

        let gas_info = BlockGasInfo::new(
            // TODO(@theochap): the gas limit should be updated dynamically `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/271`
            S::initial_gas_limit(),
            base_fee_per_gas,
        );

        self.slots
            .set(
                &slot_number,
                &SlotInformation {
                    hash: slot_header.hash(),
                    validity_condition: *validity_condition,
                    gas_info,
                },
                state,
            )
            .unwrap_infallible();

        self.time.set_true_current(&slot_header.time(), state);

        self.current_visible_hash(pre_state_root, state)
    }

    /// Updates the gas used by the transition in progress at the end of each slot
    pub fn end_slot_hook(&self, gas_used: &S::Gas, state: &mut KernelStateAccessor<S::Storage>) {
        // We retrieve the last slot in progress, update its gas information and store it back to the state
        let mut in_progress_slot = self
            .get_last_slot(state)
            .unwrap_infallible()
            .expect("There should always be a transition in progress");

        in_progress_slot.gas_info.update_gas_used(gas_used.clone());

        self.slots
            .set(&state.true_slot_number(), &in_progress_slot, state)
            .unwrap_infallible();

        self.true_to_virtual_slot_number_history
            .set(
                &state.true_slot_number(),
                &state.virtual_slot_number(),
                state,
            )
            .unwrap_infallible();
    }
}

#[cfg(feature = "native")]
impl<S: Spec> KernelWithSlotMapping<S> for ChainState<S> {
    fn visible_slot_number_at(
        &self,
        true_slot_number: u64,
        state: &mut sov_modules_api::state::ApiStateAccessor<S>,
    ) -> u64 {
        self.visible_slot_number_at(true_slot_number, state)
            .unwrap_infallible()
    }
}
