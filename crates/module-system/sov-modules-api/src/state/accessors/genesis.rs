use sov_state::{CompileTimeNamespace, EventContainer, IsValueCached, SlotKey, SlotValue, Storage};

use super::checkpoints::StateCheckpoint;
use super::seal::CachedAccessor;
use crate::state::events::TypedEvent;
use crate::{GasMeter, GasMeteringError, Genesis, KernelWriter, Spec, UnlimitedGasMeter};

/// A special state accessor which can only be used at genesis.
/// Since genesis is unproven, this state accessor may read and write to every namespace, and it is not metered.
pub struct GenesisStateAccessor<'a, S: Spec> {
    checkpoint: &'a mut StateCheckpoint<S::Storage>,
    pub(super) events: Vec<TypedEvent>,
    gas_meter: UnlimitedGasMeter<S::Gas>,
}

impl<Store: Storage> StateCheckpoint<Store> {
    /// Produces an unmetered [`GenesisStateAccessor`] from a [`StateCheckpoint`] for genesis.
    pub fn to_genesis_state_accessor<G: Genesis, S: Spec<Storage = Store>>(
        &mut self,
        // This argument prevents this method from being called outside of genesis.
        _config: &G::Config,
    ) -> GenesisStateAccessor<S> {
        GenesisStateAccessor {
            checkpoint: self,
            gas_meter: UnlimitedGasMeter::new(),
            events: Default::default(),
        }
    }
}

impl<S: Spec> KernelWriter for GenesisStateAccessor<'_, S> {
    fn true_slot_number(&self) -> u64 {
        0
    }
}

impl<'a, S: Spec, N: CompileTimeNamespace> CachedAccessor<N> for GenesisStateAccessor<'a, S> {
    fn get_cached(&mut self, key: &SlotKey) -> (Option<SlotValue>, IsValueCached) {
        CachedAccessor::<N>::get_cached(self.checkpoint, key)
    }

    fn set_cached(&mut self, key: &SlotKey, value: SlotValue) -> IsValueCached {
        CachedAccessor::<N>::set_cached(self.checkpoint, key, value)
    }

    fn delete_cached(&mut self, key: &SlotKey) -> IsValueCached {
        CachedAccessor::<N>::delete_cached(self.checkpoint, key)
    }
}

impl<'a, S: Spec> GasMeter<S::Gas> for GenesisStateAccessor<'a, S> {
    fn charge_gas(&mut self, amount: &S::Gas) -> Result<(), GasMeteringError<S::Gas>> {
        self.gas_meter.charge_gas(amount)
    }
    fn refund_gas(&mut self, gas: &S::Gas) -> Result<(), GasMeteringError<S::Gas>> {
        self.gas_meter.refund_gas(gas)
    }

    fn gas_info(&self) -> crate::GasInfo<S::Gas> {
        self.gas_meter.gas_info()
    }
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
