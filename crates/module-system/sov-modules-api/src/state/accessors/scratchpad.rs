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
    TransactionConsumption,
};
use crate::{BasicGasMeter, Gas, GasArray, GasInfo, GasMeter, GasMeteringError};

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
    pub fn to_pre_exec_working_set<Sp: Spec<Storage = S>>(
        self,
        gas_meter: BasicGasMeter<<Sp as Spec>::Gas>,
    ) -> PreExecWorkingSet<Sp> {
        let gas_info = gas_meter.gas_info();
        PreExecWorkingSet {
            starting_gas: gas_info.gas_used.clone(),
            inner: self,
            gas_meter,
        }
    }
}

/// A working set that can be used to charge gas for pre transaction execution checks.
pub struct PreExecWorkingSet<S: Spec> {
    inner: TxScratchpad<S::Storage>,
    gas_meter: BasicGasMeter<S::Gas>,
    starting_gas: S::Gas,
}

impl<S: Spec> PreExecWorkingSet<S> {
    /// Returns the associated gas meter and the scratchpad.
    pub fn to_scratchpad_and_gas_meter(self) -> (TxScratchpad<S::Storage>, BasicGasMeter<S::Gas>) {
        (self.inner, self.gas_meter)
    }

    /// Starts recording the gas usage.
    pub fn start_recording_gas_usage(&mut self) {
        let gas_info = self.gas_meter.gas_info();
        self.starting_gas = gas_info.gas_used;
    }

    /// Gets the gas usage.
    pub fn get_recorded_gas_usage(&self) -> S::Gas {
        let gas_info = self.gas_meter.gas_info();
        let end_gas = &gas_info.gas_used;
        end_gas.checked_sub(&self.starting_gas).expect(
            "Gas used should be greater than starting gas, PreExecWorkingSet never refunds gas.",
        )
    }
}

impl<S: Spec> GasMeter<S::Gas> for PreExecWorkingSet<S> {
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

impl<S: Spec> CachedAccessor<User> for PreExecWorkingSet<S> {
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
            gas_meter: BasicGasMeter::new(u64::MAX, <S::Gas as crate::Gas>::Price::ZEROED),
            max_fee: 0,
            max_priority_fee_bips: PriorityFeeBips::ZERO,
        }
    }
}

/// This structure contains the read-write set and the events collected during the execution of a transaction.
/// There are two ways to convert it into a StateCheckpoint:
/// 1. By using the [`WorkingSet::finalize`] method, where all the changes are added to the underlying
/// [`TxScratchpad`].
/// 2. By using the [`WorkingSet::revert`] method, where the most recent changes are reverted and the previous [`TxScratchpad`] is returned.
pub struct WorkingSet<S: Spec> {
    pub(super) delta: RevertableWriter<TxScratchpad<S::Storage>>,
    events: Vec<TypedEvent>,
    gas_meter: BasicGasMeter<S::Gas>,
    // Gas parameters of the transaction associated with the working set
    max_fee: u64,
    max_priority_fee_bips: PriorityFeeBips,
}

impl<S: Spec> WorkingSet<S> {
    /// Creates a new [`WorkingSet`] from the provided [`TxScratchpad`] and [`AuthenticatedTransactionData`].
    #[allow(clippy::result_large_err)]
    pub fn create_working_set(
        scratchpad: TxScratchpad<S::Storage>,
        gas_price: &<S::Gas as Gas>::Price,
        tx: &AuthenticatedTransactionData<S>,
    ) -> Self {
        let working_set_gas_meter = tx.gas_meter(gas_price);

        Self {
            delta: RevertableWriter::new(scratchpad),
            events: Default::default(),
            gas_meter: working_set_gas_meter,
            max_fee: tx.max_fee,
            max_priority_fee_bips: tx.max_priority_fee_bips,
        }
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
            gas_meter: BasicGasMeter::new(remaining_funds, price.clone()),
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
    pub fn new_deprecated<K: Kernel<S>>(inner: S::Storage, kernel: &K) -> Self {
        let state_checkpoint: StateCheckpoint<S::Storage> = StateCheckpoint::new(inner, kernel);
        let tx_scratchpad = TxScratchpad {
            inner: RevertableWriter::new(state_checkpoint),
        };

        WorkingSet {
            delta: RevertableWriter::new(tx_scratchpad),
            events: Default::default(),
            gas_meter: BasicGasMeter::new(u64::MAX, <S::Gas as crate::Gas>::Price::ZEROED),
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

#[cfg(test)]
mod tests {
    use sov_test_utils::storage::new_finalized_storage;
    use sov_test_utils::{MockDaSpec, MockZkVerifier};

    use super::*;
    use crate::execution_mode::Native;
    type TestSpec =
        crate::default_spec::DefaultSpec<MockDaSpec, MockZkVerifier, MockZkVerifier, Native>;

    use crate::capabilities::mocks::MockKernel;
    use crate::{PreExecWorkingSet, Spec};

    #[test]
    fn test_gas_recording() {
        let tmpdir = tempfile::tempdir().unwrap();
        let storage = new_finalized_storage(tmpdir.path());
        let kernel = MockKernel::<TestSpec>::default();
        let state: StateCheckpoint<<TestSpec as Spec>::Storage> =
            StateCheckpoint::new(storage, &kernel);

        let state = state.to_tx_scratchpad();

        let gas_price = <<TestSpec as Spec>::Gas as Gas>::Price::from([4; 2]);
        let starting_funds = 10000;
        let mut pre_exec_ws = PreExecWorkingSet::<TestSpec> {
            inner: state,
            starting_gas: <TestSpec as Spec>::Gas::ZEROED,
            gas_meter: BasicGasMeter::new(starting_funds, gas_price.clone()),
        };

        let mut total_cost = 0;
        for _ in 0..10 {
            pre_exec_ws.start_recording_gas_usage();
            charge_gas(&mut pre_exec_ws);
            let gas_used = pre_exec_ws.get_recorded_gas_usage();
            total_cost += gas_used.value(&gas_price);
        }

        let gas_info = pre_exec_ws.gas_meter.gas_info();
        assert_eq!(gas_info.remaining_funds, starting_funds - total_cost);
    }

    fn charge_gas(pre_exec_ws: &mut PreExecWorkingSet<TestSpec>) {
        let gas = <TestSpec as Spec>::Gas::from([5; 2]);
        pre_exec_ws.charge_gas(&gas).unwrap();
    }
}
