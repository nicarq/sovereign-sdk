//! Implements the `SlotHooks` trait for the `ValueSetter` module.
//!
//! These hook simply count the number of times they are called. This is useful for testing and debugging.

use sov_modules_api::prelude::UnwrapInfallible;
#[cfg(feature = "native")]
use sov_modules_api::{AccessoryStateReaderAndWriter, FinalizeHook};
use sov_modules_api::{SlotHooks, Spec, StateCheckpoint};
use sov_state::Storage;

use crate::ValueSetter;

impl<S: Spec> SlotHooks for ValueSetter<S> {
    type Spec = S;
    /// Hook that runs at the beginning of the `apply_slot` function inside the `StateTransitionFunction`.
    fn begin_slot_hook(
        &self,
        _visible_hash: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        state: &mut StateCheckpoint<Self::Spec>,
    ) {
        let next_value = self
            .begin_slot_hook_count
            .get(state)
            .unwrap_infallible()
            .unwrap_or(0)
            + 1;
        self.begin_slot_hook_count
            .set(&next_value, state)
            .unwrap_infallible();
    }

    /// Hook that runs at the end of the `apply_slot` function inside the `StateTransitionFunction`.
    fn end_slot_hook(&self, state: &mut StateCheckpoint<Self::Spec>) {
        let next_value = self
            .end_slot_hook_count
            .get(state)
            .unwrap_infallible()
            .unwrap_or(0)
            + 1;
        self.end_slot_hook_count
            .set(&next_value, state)
            .unwrap_infallible();
    }
}

#[cfg(feature = "native")]
impl<S: Spec> FinalizeHook for ValueSetter<S> {
    type Spec = S;
    fn finalize_hook(
        &self,
        _root_hash: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        state: &mut impl AccessoryStateReaderAndWriter,
    ) {
        let next_value = self
            .finalize_hook_count
            .get(state)
            .unwrap_infallible()
            .unwrap_or(0)
            + 1;
        self.finalize_hook_count
            .set(&next_value, state)
            .unwrap_infallible();
    }
}
