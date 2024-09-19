use sov_modules_api::da::BlockHeaderTrait;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{DaSpec, KernelStateAccessor, KernelWriter, Spec};
use sov_state::{StateRoot, Storage};

use crate::{BlockGasInfo, ChainState, StateTransition, TransitionInProgress};

impl<S: Spec, Da: DaSpec> ChainState<S, Da> {
    /// Computes the current root hash available at the current *virtual* slot number.
    /// This is the kernel root hash at the *virtual* rollup height with the user root hash at the current height.
    fn current_visible_hash(
        &self,
        pre_state_root: &<S::Storage as Storage>::Root,
        state: &mut KernelStateAccessor<S::Storage>,
    ) -> <S::Storage as Storage>::Root {
        let user_root = pre_state_root.namespace_root(sov_state::ProvableNamespace::User);

        let kernel_root = if let Some(transition) = self
            .get_historical_transitions(state.virtual_slot_number().saturating_sub(1), state)
            .unwrap_infallible()
        {
            transition.post_state_root().clone()
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
        slot_header: &Da::BlockHeader,
        validity_condition: &Da::ValidityCondition,
        pre_state_root: &<S::Storage as Storage>::Root,
        state: &mut KernelStateAccessor<S::Storage>,
    ) -> <S::Storage as Storage>::Root {
        let gas_info = if self.genesis_root.get(state).unwrap_infallible().is_none() {
            // The genesis hash is not set, hence this is the
            // first transition right after the genesis block
            self.genesis_root
                .set(pre_state_root, state)
                .unwrap_infallible();

            BlockGasInfo::new(Self::initial_gas_limit(), Self::initial_base_fee_per_gas())
        } else {
            let transition: StateTransition<S, Da> = {
                let TransitionInProgress {
                    slot_hash,
                    validity_condition,
                    gas_info,
                } = self
                    .get_in_progress_transition_prev_slot(state)
                    .expect("There should always be a transition in progress");

                StateTransition {
                    slot_hash,
                    post_state_root: pre_state_root.clone(),
                    validity_condition,
                    gas_info,
                }
            };

            let slot_number = self.true_slot_number(state).unwrap_infallible();
            self.historical_transitions
                .set(
                    &(slot_number
                        .checked_sub(1)
                        .expect("Trying to set a transition at a negative rollup height")),
                    &transition,
                    state,
                )
                .unwrap_infallible();

            // The base fee per gas is updated according to the EIP-1559 specification
            let computed_base_fee = Self::compute_base_fee_per_gas(&transition.gas_info);

            BlockGasInfo::new(
                // TODO(@theochap): the gas limit should be updated dynamically `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/271`
                Self::initial_gas_limit(),
                computed_base_fee,
            )
        };

        self.time.set_true_current(&slot_header.time(), state);

        self.in_progress_transition.set_true_current(
            &TransitionInProgress {
                slot_hash: slot_header.hash(),
                validity_condition: *validity_condition,
                gas_info,
            },
            state,
        );

        self.current_visible_hash(pre_state_root, state)
    }

    /// Updates the gas used by the transition in progress at the end of each slot
    pub fn end_slot_hook(
        &self,
        gas_used: &S::Gas,
        state: &mut KernelStateAccessor<S::Storage>,
    ) -> Option<[u8; 32]> {
        let mut in_progress_transition = self
            .in_progress_transition
            .get(&(state.true_slot_number()), state)
            .unwrap_infallible()
            .expect("There should always be a transition in progress");

        in_progress_transition
            .gas_info
            .update_gas_used(gas_used.clone());

        self.in_progress_transition
            .set_true_current(&in_progress_transition, state);

        // Soft confirmations:
        // - if the current virtual slot is behind the true slot number, kernel root computed at the end of the current slot
        // should not be visible. We should return the kernel root that was computed when the true height was equal to the virtual height.
        // - if the current virtual slot is equal to the true slot number, we cannot know the next kernel root yet, so we return None and we
        // use the root computed when the state gets commited as a visible root.
        let kernel_root = if state.virtual_slot_number() == 0 {
            Some(
                self.genesis_root
                    .get(state)
                    .unwrap_infallible()
                    .expect("Genesis height should always be set.")
                    .namespace_root(sov_state::ProvableNamespace::Kernel),
            )
        } else {
            self.get_historical_transitions(state.virtual_slot_number(), state)
                .unwrap_infallible()
                .map(|transition| {
                    transition
                        .post_state_root()
                        .namespace_root(sov_state::ProvableNamespace::Kernel)
                })
        };

        // We increment the slot number at the very end of the slot execution
        self.increment_true_slot_number(state);

        kernel_root
    }
}
