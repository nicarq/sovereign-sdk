use sov_modules_api::{Context, DaSpec, KernelWorkingSet, StateValueAccessor};

use crate::{ChainState, TransitionInProgress};

impl<C: Context, Da: DaSpec> TransitionInProgress<C, Da> {
    /// Overrides the gas used for a transition
    pub fn override_gas_used(&mut self, gas_used: C::Gas) {
        self.gas_used = gas_used;
    }
}

impl<C: Context, Da: DaSpec> ChainState<C, Da> {
    /// Overrides the in progress tx data
    pub fn override_in_progress_transition(
        &self,
        transition: TransitionInProgress<C, Da>,
        working_set: &mut KernelWorkingSet<C>,
    ) {
        self.in_progress_transition.set(&transition, working_set);
    }
}
