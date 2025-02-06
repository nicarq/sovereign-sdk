use sov_rollup_interface::common::{SlotNumber, VisibleSlotNumber};
use sov_state::{EventContainer, IsValueCached, SlotKey, SlotValue};

use super::checkpoints::StateCheckpoint;
use super::UniversalStateAccessor;
use crate::capabilities::RollupHeight;
use crate::state::events::TypedEvent;
use crate::{GasMeter, Genesis, KernelWriter, Spec, VersionReader};

/// A special state accessor which can only be used at genesis.
/// Since genesis is unproven, this state accessor may read and write to every namespace, and it is not metered.
pub struct GenesisStateAccessor<'a, S: Spec> {
    checkpoint: &'a mut StateCheckpoint<S>,
    pub(super) events: Vec<TypedEvent>,
}

impl<S: Spec> StateCheckpoint<S> {
    /// Produces an unmetered [`GenesisStateAccessor`] from a [`StateCheckpoint`] for genesis.
    pub fn to_genesis_state_accessor<G: Genesis>(
        &mut self,
        // This argument prevents this method from being called outside of genesis.
        _config: &G::Config,
    ) -> GenesisStateAccessor<S> {
        GenesisStateAccessor {
            checkpoint: self,
            events: Default::default(),
        }
    }
}

impl<S: Spec> KernelWriter for GenesisStateAccessor<'_, S> {
    fn true_slot_number(&self) -> SlotNumber {
        SlotNumber::GENESIS
    }
}

impl<S: Spec> VersionReader for GenesisStateAccessor<'_, S> {
    fn visible_slot_number_to_access(&self) -> VisibleSlotNumber {
        VisibleSlotNumber::GENESIS
    }

    fn rollup_height_to_access(&self) -> RollupHeight {
        RollupHeight::GENESIS
    }
}

impl<'a, S: Spec> UniversalStateAccessor for GenesisStateAccessor<'a, S> {
    fn is_value_cached(&self, namespace: sov_state::Namespace, key: &SlotKey) -> IsValueCached {
        self.checkpoint.is_value_cached(namespace, key)
    }

    fn get_size(&mut self, namespace: sov_state::Namespace, key: &SlotKey) -> Option<u64> {
        self.checkpoint.get_size(namespace, key)
    }

    fn get_value(&mut self, namespace: sov_state::Namespace, key: &SlotKey) -> Option<SlotValue> {
        self.checkpoint.get_value(namespace, key)
    }

    fn set_value(&mut self, namespace: sov_state::Namespace, key: &SlotKey, value: SlotValue) {
        self.checkpoint.set_value(namespace, key, value);
    }

    fn delete_value(&mut self, namespace: sov_state::Namespace, key: &SlotKey) {
        self.checkpoint.delete_value(namespace, key);
    }
}

impl<'a, S: Spec> GasMeter for GenesisStateAccessor<'a, S> {
    type Spec = S;
}

impl<'a, S: Spec> GenesisStateAccessor<'a, S> {
    /// Extracts all typed events from this working set.
    pub fn take_events(&mut self) -> Vec<TypedEvent> {
        core::mem::take(&mut self.events)
    }

    /// Extracts a typed event at index `index`
    pub fn take_event(&mut self, index: usize) -> Option<TypedEvent> {
        if index < self.events.len() {
            Some(self.events.remove(index))
        } else {
            None
        }
    }

    /// Returns an immutable map of all typed events that have been previously
    /// written to this working set.
    pub fn events(&self) -> &[TypedEvent] {
        &self.events
    }
}

impl<'a, S: Spec> EventContainer for GenesisStateAccessor<'a, S> {
    fn add_event<E: 'static + core::marker::Send>(&mut self, event_key: &str, event: E) {
        self.events.push(TypedEvent::new(event_key, event));
    }
}

use crate::GenesisState;
impl<'a, S: Spec> GenesisState<S> for GenesisStateAccessor<'a, S> {}
