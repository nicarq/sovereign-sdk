use sov_modules_api::da::BlockHeaderTrait;
use sov_modules_api::{Gas, Spec};
use sov_state::storage::KernelWorkingSet;
use sov_state::Storage;

use crate::{ChainState, StateTransition, TransitionInProgress};

impl<S: Spec, Da: sov_modules_api::DaSpec> ChainState<S, Da> {
    /// Update the chain state at the beginning of the slot. Compute the next gas price
    pub fn begin_slot_hook(
        &self,
        slot_header: &Da::BlockHeader,
        validity_condition: &Da::ValidityCondition,
        pre_state_root: &<S::Storage as Storage>::Root,
        working_set: &mut KernelWorkingSet<S>,
    ) -> <S::Gas as Gas>::Price {
        if self.genesis_hash.get(working_set.inner).is_none() {
            // The genesis hash is not set, hence this is the
            // first transition right after the genesis block
            self.genesis_hash.set(pre_state_root, working_set.inner);
        } else {
            let transition: StateTransition<S, Da> = {
                let TransitionInProgress {
                    slot_hash,
                    validity_condition,
                    gas_price,
                    gas_used,
                } = self
                    .in_progress_transition
                    .get_current(working_set)
                    .expect("There should always be a transition in progress");

                StateTransition {
                    slot_hash,
                    post_state_root: pre_state_root.clone(),
                    validity_condition,
                    gas_used,
                    gas_price,
                }
            };

            let slot_number = self.true_slot_number(working_set);
            self.historical_transitions
                .set(&slot_number, &transition, working_set.inner);
        }

        // Since we increment the true slot number, we have to update the working set.
        self.increment_true_slot_number(working_set);

        self.time.set_true_current(&slot_header.time(), working_set);

        let slot_number = self.true_slot_number(working_set);

        let gas_price_state = self
            .get_gas_price_state(working_set.inner)
            .expect("the gas price state will be available from genesis")
            .update(slot_number, &self.historical_transitions, working_set.inner)
            .expect("the transition data must be available");

        self.in_progress_transition.set_true_current(
            &TransitionInProgress {
                slot_hash: slot_header.hash(),
                validity_condition: *validity_condition,
                gas_price: gas_price_state.price.clone(),
                gas_used: S::Gas::zero(),
            },
            working_set,
        );
        gas_price_state.price
    }

    /// Update the chain state at the end of each slot, if necessary
    pub fn end_slot_hook(&self, gas_used: &S::Gas, working_set: &mut KernelWorkingSet<S>) {
        let mut in_progress_transition = self
            .in_progress_transition
            .get_current(working_set)
            .expect("There should always be a transition in progress");

        in_progress_transition.gas_used = gas_used.clone();
        self.in_progress_transition
            .set_true_current(&in_progress_transition, working_set);
    }
}
