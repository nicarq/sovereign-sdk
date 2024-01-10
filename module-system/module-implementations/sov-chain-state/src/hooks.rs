use sov_modules_api::da::BlockHeaderTrait;
use sov_modules_api::hooks::FinalizeHook;
use sov_modules_api::prelude::*;
use sov_modules_api::{AccessoryWorkingSet, Context, GasUnit, Spec};
use sov_state::storage::KernelWorkingSet;
use sov_state::Storage;

use crate::{ChainState, StateTransitionId, TransitionInProgress};

impl<C: Context, Da: sov_modules_api::DaSpec> ChainState<C, Da> {
    /// Update the chain state at the beginning of the slot
    pub fn begin_slot_hook(
        &self,
        slot_header: &Da::BlockHeader,
        validity_condition: &Da::ValidityCondition,
        pre_state_root: &<<C as Spec>::Storage as Storage>::Root,
        working_set: &mut KernelWorkingSet<C>,
    ) {
        if self.genesis_hash.get(working_set.inner).is_none() {
            // The genesis hash is not set, hence this is the
            // first transition right after the genesis block
            self.genesis_hash.set(pre_state_root, working_set.inner)
        } else {
            let transition: StateTransitionId<C, Da> = {
                let TransitionInProgress {
                    da_block_hash,
                    validity_condition,
                    gas_price,
                    gas_used,
                } = self
                    .in_progress_transition
                    .get(working_set)
                    .expect("There should always be a transition in progress");

                StateTransitionId {
                    da_block_hash,
                    post_state_root: pre_state_root.clone(),
                    validity_condition,
                    gas_used,
                    gas_price,
                }
            };

            let height = self.true_slot_height(working_set.inner);
            self.store_state_transition(height, transition, working_set.inner);
        }

        self.increment_true_slot_height(working_set);
        self.time.set_current(&slot_header.time(), working_set);

        let genesis_height = self
            .genesis_height
            .get(working_set.inner)
            .expect("the genesis height is part of the module initialization");
        let height = self.true_slot_height(working_set.inner);

        let gas_price_state = self
            .get_gas_price_state(working_set.inner)
            .expect("the gas price state will be available from genesis")
            .update(
                genesis_height,
                height,
                &self.historical_transitions,
                working_set.inner,
            )
            .expect("the transition data must be available");

        self.in_progress_transition.set(
            &TransitionInProgress {
                da_block_hash: slot_header.hash(),
                validity_condition: *validity_condition,
                gas_price: gas_price_state.price.clone(),
                gas_used: C::GasUnit::ZEROED,
            },
            working_set,
        );
    }

    /// Update the chain state at the end of each slot, if necessary
    pub fn end_slot_hook(&self, working_set: &mut KernelWorkingSet<C>) {
        let mut in_progress_transition = self
            .in_progress_transition
            .get(working_set)
            .expect("There should always be a transition in progress");

        in_progress_transition.gas_used = working_set.inner.gas_used().clone();
        self.in_progress_transition
            .set(&in_progress_transition, working_set);
    }
}

impl<C: Context, Da: sov_modules_api::DaSpec> FinalizeHook<Da> for ChainState<C, Da> {
    type Context = C;

    fn finalize_hook(
        &self,
        _root_hash: &<C::Storage as Storage>::Root,
        _accesorry_working_set: &mut AccessoryWorkingSet<C>,
    ) {
    }
}
