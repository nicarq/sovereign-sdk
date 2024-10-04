//! Runtime state machine definitions.

use sov_state::namespaces::User;
use sov_state::{
    CompileTimeNamespace, EventContainer, IsValueCached, Namespace, SlotKey, SlotValue, Storage,
};

use super::checkpoints::StateCheckpoint;
use super::internals::RevertableWriter;
use super::seal::CachedAccessor;
use super::UniversalStateAccessor;
#[cfg(feature = "test-utils")]
use crate::capabilities::Kernel;
use crate::module::Spec;
use crate::state::events::TypedEvent;
use crate::transaction::{
    transaction_consumption_helper, AuthenticatedTransactionData, PriorityFeeBips,
    TransactionConsumption, TxGasMeter,
};
#[cfg(feature = "test-utils")]
use crate::UnlimitedGasMeter;
use crate::{GasInfo, GasMeter, GasMeteringError};

/// A state diff over the storage that contains all the changes related to transaction execution.
/// This structure is built from a [`StateCheckpoint`] and is used in the entire transaction lifecycle (from
/// pre-execution checks to post execution state updates).
///
/// ## Usage note
/// This method tracks the gas consumed outside of the transaction lifecycle without explicitely consuming a finite resource.
/// This should only be used in infailible methods.
pub struct TxScratchpad<S: Storage> {
    inner: RevertableWriter<StateCheckpoint<S>>,
}

impl<S: Storage> StateCheckpoint<S> {
    /// Transforms this [`StateCheckpoint`] into a [`TxScratchpad`].
    pub fn to_tx_scratchpad(self) -> TxScratchpad<S> {
        TxScratchpad::<S> {
            inner: RevertableWriter::new(self),
        }
    }
}

impl<S: Storage> UniversalStateAccessor for TxScratchpad<S> {
    fn get(&mut self, namespace: Namespace, key: &SlotKey) -> (Option<SlotValue>, IsValueCached) {
        <RevertableWriter<StateCheckpoint<S>> as UniversalStateAccessor>::get(
            &mut self.inner,
            namespace,
            key,
        )
    }

    fn set(&mut self, namespace: Namespace, key: &SlotKey, value: SlotValue) -> IsValueCached {
        <RevertableWriter<StateCheckpoint<S>> as UniversalStateAccessor>::set(
            &mut self.inner,
            namespace,
            key,
            value,
        )
    }

    fn delete(&mut self, namespace: Namespace, key: &SlotKey) -> IsValueCached {
        <RevertableWriter<StateCheckpoint<S>> as UniversalStateAccessor>::delete(
            &mut self.inner,
            namespace,
            key,
        )
    }
}

impl<S: Storage> TxScratchpad<S> {
    /// Commits the changes of this [`TxScratchpad`] and returns a [`StateCheckpoint`].
    pub fn commit(self) -> StateCheckpoint<S> {
        self.inner.commit()
    }

    /// Reverts the changes of this [`TxScratchpad`] and returns a [`StateCheckpoint`].
    pub fn revert(self) -> StateCheckpoint<S> {
        self.inner.revert()
    }

    /// Converts this [`TxScratchpad`] into a [`PreExecWorkingSet`].
    pub fn to_pre_exec_working_set<Sp: Spec<Storage = S>, Meter: GasMeter<Sp::Gas>>(
        self,
        gas_meter: Meter,
    ) -> PreExecWorkingSet<Sp, Meter> {
        PreExecWorkingSet {
            inner: self,
            gas_meter,
        }
    }
}

#[cfg(feature = "test-utils")]
impl<Store: Storage> TxScratchpad<Store> {
    /// Produces an unmetered [`PreExecWorkingSet`] from this [`StateCheckpoint`].
    /// This is useful for tests that don't need to track gas consumption.
    pub fn pre_exec_ws_unmetered<S: Spec<Storage = Store>>(
        self,
    ) -> PreExecWorkingSet<S, UnlimitedGasMeter<S::Gas>> {
        PreExecWorkingSet {
            inner: self,
            gas_meter: UnlimitedGasMeter::new(),
        }
    }

    /// Produces an unmetered [`PreExecWorkingSet`] from this [`StateCheckpoint`] for a given price.
    /// This is useful for tests that don't need to test failure over gas exhaustion.
    pub fn pre_exec_ws_unmetered_with_price<S: Spec<Storage = Store>>(
        self,
        gas_price: &<S::Gas as crate::Gas>::Price,
    ) -> PreExecWorkingSet<S, UnlimitedGasMeter<S::Gas>> {
        PreExecWorkingSet {
            inner: self,
            gas_meter: UnlimitedGasMeter::new_with_price(gas_price.clone()),
        }
    }
}

/// A working set that can be used to charge gas for pre transaction execution checks.
pub struct PreExecWorkingSet<S: Spec, PreExecChecksMeter: GasMeter<S::Gas>> {
    inner: TxScratchpad<S::Storage>,
    gas_meter: PreExecChecksMeter,
}

impl<S: Spec, PreExecChecksMeter: GasMeter<S::Gas>> PreExecWorkingSet<S, PreExecChecksMeter> {
    /// Returns the associated gas meter and the scratchpad.
    pub fn to_scratchpad_and_gas_meter(self) -> (TxScratchpad<S::Storage>, PreExecChecksMeter) {
        (self.inner, self.gas_meter)
    }
}

impl<S: Spec, Meter: GasMeter<S::Gas>> GasMeter<S::Gas> for PreExecWorkingSet<S, Meter> {
    fn charge_gas(&mut self, amount: &S::Gas) -> anyhow::Result<(), GasMeteringError<S::Gas>> {
        self.gas_meter.charge_gas(amount)
    }

    fn refund_gas(&mut self, gas: &S::Gas) -> anyhow::Result<(), GasMeteringError<S::Gas>> {
        self.gas_meter.refund_gas(gas)
    }

    fn gas_info(&self) -> GasInfo<S::Gas> {
        self.gas_meter.gas_info()
    }
}

impl<S: Spec, Meter: GasMeter<S::Gas>> CachedAccessor<User> for PreExecWorkingSet<S, Meter> {
    fn get_cached(&mut self, key: &SlotKey) -> (Option<SlotValue>, IsValueCached) {
        <TxScratchpad<S::Storage> as CachedAccessor<User>>::get_cached(&mut self.inner, key)
    }

    fn set_cached(&mut self, key: &SlotKey, value: SlotValue) -> IsValueCached {
        <TxScratchpad<S::Storage> as CachedAccessor<User>>::set_cached(&mut self.inner, key, value)
    }

    fn delete_cached(&mut self, key: &SlotKey) -> IsValueCached {
        <TxScratchpad<S::Storage> as CachedAccessor<User>>::delete_cached(&mut self.inner, key)
    }
}

#[cfg(feature = "test-utils")]
impl<Store: Storage> StateCheckpoint<Store> {
    /// Produces an unmetered [`WorkingSet`] from this [`StateCheckpoint`].
    /// This is useful for tests that don't need to track gas consumption.
    pub fn to_working_set_unmetered<S: Spec<Storage = Store>>(self) -> WorkingSet<S> {
        let stashed_working_set = TxScratchpad {
            inner: RevertableWriter::new(self),
        };

        WorkingSet {
            delta: RevertableWriter::new(stashed_working_set),
            events: Default::default(),
            gas_meter: TxGasMeter::unmetered(),
            max_fee: 0,
            max_priority_fee_bips: PriorityFeeBips::ZERO,
        }
    }
}

/// Error type that can be raised by the [`WorkingSet::try_create_working_set`] method.
pub struct NotEnoughGasError<S: Spec> {
    pub scratchpad: TxScratchpad<S::Storage>,
    pub reason: String,
}

/// This structure contains the read-write set and the events collected during the execution of a transaction.
/// There are two ways to convert it into a StateCheckpoint:
/// 1. By using the [`WorkingSet::finalize`] method, where all the changes are added to the underlying
/// [`TxScratchpad`].
/// 2. By using the [`WorkingSet::revert`] method, where the most recent changes are reverted and the previous [`TxScratchpad`] is returned.
pub struct WorkingSet<S: Spec> {
    pub(super) delta: RevertableWriter<TxScratchpad<S::Storage>>,
    events: Vec<TypedEvent>,
    gas_meter: TxGasMeter<S::Gas>,
    // Gas parameters of the transaction associated with the working set
    max_fee: u64,
    max_priority_fee_bips: PriorityFeeBips,
}

impl<S: Spec> WorkingSet<S> {
    /// Creates a new [`WorkingSet`] from the provided [`TxScratchpad`] and [`AuthenticatedTransactionData`].
    /// The working set will allocate gas according to the transaction's data, minus the gas consumed by pre-execution checks.
    #[allow(clippy::result_large_err)]
    pub fn try_create_working_set(
        scratchpad: TxScratchpad<S::Storage>,
        gas_info: &GasInfo<S::Gas>,
        tx: &AuthenticatedTransactionData<S>,
    ) -> Result<Self, NotEnoughGasError<S>> {
        let mut working_set_gas_meter = tx.gas_meter(&gas_info.gas_price);
        if let Err(e) = working_set_gas_meter.charge_gas(&gas_info.gas_used) {
            return Err(NotEnoughGasError {
                scratchpad,
                reason: e.to_string(),
            });
        }

        Ok(Self {
            delta: RevertableWriter::new(scratchpad),
            events: Default::default(),
            gas_meter: working_set_gas_meter,
            max_fee: tx.max_fee,
            max_priority_fee_bips: tx.max_priority_fee_bips,
        })
    }

    /// Builds a [`crate::TransactionConsumption`] from the [`WorkingSet`].
    pub(crate) fn transaction_consumption(&self) -> TransactionConsumption<S::Gas> {
        // The base fee is the amount of gas consumed by the transaction execution.
        let base_fee = self.gas_meter.gas_info().gas_used;
        let gas_price = self.gas_meter.gas_info().gas_price;

        transaction_consumption_helper::<S>(
            &base_fee,
            &gas_price,
            self.max_fee,
            self.max_priority_fee_bips,
        )
    }

    /// Turns this [`WorkingSet`] into a [`TxScratchpad`], commits the changes to the [`WorkingSet`] to the
    /// inner scratchpad.
    pub fn finalize(
        self,
    ) -> (
        TxScratchpad<S::Storage>,
        TransactionConsumption<S::Gas>,
        Vec<TypedEvent>,
    ) {
        let tx_reward = self.transaction_consumption();
        (self.delta.commit(), tx_reward, self.events)
    }

    /// Reverts the most recent changes to this [`WorkingSet`], returning a pristine
    /// [`TxScratchpad`] instance.
    pub fn revert(self) -> (TxScratchpad<S::Storage>, TransactionConsumption<S::Gas>) {
        let tx_consumption = self.transaction_consumption();
        (self.delta.revert(), tx_consumption)
    }

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

    /// Returns the remaining gas funds.
    pub fn gas_remaining_funds(&self) -> u64 {
        self.gas_meter.gas_info().remaining_funds
    }

    /// Returns the maximum fee that can be paid for this transaction expressed in gas token amount.
    pub fn max_fee(&self) -> u64 {
        self.max_fee
    }

    /// A helper function to create a new [`WorkingSet`] with a given gas price and remaining funds.
    /// Note: This method uses a [`MockKernel`] with a default height, this is not compatible with tests over multiple slots.
    #[cfg(test)]
    pub fn new_with_gas_meter(
        inner: S::Storage,
        remaining_funds: u64,
        price: &<S::Gas as crate::Gas>::Price,
    ) -> Self {
        use crate::capabilities::mocks::MockKernel;

        let state_checkpoint: StateCheckpoint<S::Storage> =
            StateCheckpoint::new(inner, &MockKernel::<S>::default());
        let tx_scratchpad = TxScratchpad {
            inner: RevertableWriter::new(state_checkpoint),
        };

        WorkingSet {
            delta: RevertableWriter::new(tx_scratchpad),
            events: Default::default(),
            gas_meter: TxGasMeter::new(remaining_funds, price.clone()),
            max_fee: 0,
            max_priority_fee_bips: PriorityFeeBips::ZERO,
        }
    }
}

#[cfg(feature = "test-utils")]
impl<S: Spec> WorkingSet<S> {
    /// Creates a new [`WorkingSet`] instance backed by the given [`Spec::Storage`].
    ///
    /// ## Deprecated(@theochap)
    /// This method is deprecated and will be removed in the future. Please refrain from writing
    /// tests that use this method.
    /// Instead, one could use (in decreasing order of preference):
    /// - the testing framework,
    /// - or [`crate::ApiStateAccessor::new`]
    /// - or [`StateCheckpoint::new`]
    pub fn new_deprecated<K: Kernel<S::Storage>>(inner: S::Storage, kernel: &K) -> Self {
        let state_checkpoint: StateCheckpoint<S::Storage> = StateCheckpoint::new(inner, kernel);
        let tx_scratchpad = TxScratchpad {
            inner: RevertableWriter::new(state_checkpoint),
        };

        WorkingSet {
            delta: RevertableWriter::new(tx_scratchpad),
            events: Default::default(),
            gas_meter: TxGasMeter::unmetered(),
            max_fee: 0,
            max_priority_fee_bips: PriorityFeeBips::ZERO,
        }
    }

    /// Turns this [`WorkingSet`] into a [`StateCheckpoint`], in preparation for
    /// committing the changes to the underlying [`Spec::Storage`] via
    /// [`StateCheckpoint::freeze`].
    ///
    /// ## Safety note
    /// This function calls [`WorkingSet::finalize`] under the hood, please be sure that we can skip this
    /// intermediary committing step. This function is only accessible in tests
    pub fn checkpoint(
        self,
    ) -> (
        StateCheckpoint<S::Storage>,
        TransactionConsumption<S::Gas>,
        Vec<TypedEvent>,
    ) {
        let (tx_scratchpad, transaction_consumption, events) = self.finalize();
        let checkpoint = tx_scratchpad.commit();

        (checkpoint, transaction_consumption, events)
    }
}

impl<S: Spec> GasMeter<S::Gas> for WorkingSet<S> {
    fn charge_gas(&mut self, gas: &S::Gas) -> Result<(), GasMeteringError<S::Gas>> {
        self.gas_meter.charge_gas(gas)
    }

    fn refund_gas(&mut self, gas: &S::Gas) -> Result<(), GasMeteringError<S::Gas>> {
        self.gas_meter.refund_gas(gas)
    }

    fn gas_info(&self) -> GasInfo<S::Gas> {
        self.gas_meter.gas_info()
    }
}

impl<S: Spec, N: CompileTimeNamespace> CachedAccessor<N> for WorkingSet<S> {
    fn get_cached(&mut self, key: &SlotKey) -> (Option<SlotValue>, IsValueCached) {
        CachedAccessor::<N>::get_cached(&mut self.delta, key)
    }
    fn set_cached(&mut self, key: &SlotKey, value: SlotValue) -> IsValueCached {
        CachedAccessor::<N>::set_cached(&mut self.delta, key, value)
    }

    fn delete_cached(&mut self, key: &SlotKey) -> IsValueCached {
        CachedAccessor::<N>::delete_cached(&mut self.delta, key)
    }
}

impl<S: Spec> EventContainer for WorkingSet<S> {
    fn add_event<E: 'static + core::marker::Send>(&mut self, event_key: &str, event: E) {
        self.events.push(TypedEvent::new(event_key, event));
    }
}
