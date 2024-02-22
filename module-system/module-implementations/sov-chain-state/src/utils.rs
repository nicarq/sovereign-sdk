use sov_modules_api::{DaSpec, KernelWorkingSet, Spec, StateValueAccessor};

use crate::{ChainState, TransitionInProgress};

impl<S: Spec, Da: DaSpec> TransitionInProgress<S, Da> {
    /// Overrides the gas used for a transition
    pub fn override_gas_used(&mut self, gas_used: S::Gas) {
        self.gas_used = gas_used;
    }
}

impl<S: Spec, Da: DaSpec> ChainState<S, Da> {
    /// Overrides the in progress tx data
    pub fn override_in_progress_transition(
        &self,
        transition: TransitionInProgress<S, Da>,
        working_set: &mut KernelWorkingSet<S>,
    ) {
        self.in_progress_transition.set(&transition, working_set);
    }
}
