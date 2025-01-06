//! Runtime state machine definitions.

use std::marker::PhantomData;

use sov_rollup_interface::common::SlotNumber;
use sov_state::{EventContainer, IsValueCached, Namespace, SlotKey, SlotValue};

use super::checkpoints::StateCheckpoint;
use super::internals::RevertableWriter;
use super::{StateProvider, UniversalStateAccessor};
use crate::module::Spec;
use crate::state::events::TypedEvent;
use crate::transaction::{
    transaction_consumption_helper, AuthenticatedTransactionData, PriorityFeeBips,
    TransactionConsumption,
};
#[cfg(feature = "test-utils")]
use crate::GasArray;
use crate::{BasicGasMeter, Gas, GasInfo, GasMeter, GasMeteringError, VersionReader};

/// A state diff over the storage that contains all the changes related to transaction execution.
///
/// This structure is built from a [`StateProvider`] (typically a
/// [`StateCheckpoint`]) and is used in the entire transaction lifecycle (from
/// pre-execution checks to post execution state updates).
///
/// ## Usage note
/// This method tracks the gas consumed outside of the transaction lifecycle without explicitely consuming a finite resource.
/// This should only be used in infailible methods.
pub struct TxScratchpad<S: Spec, I: StateProvider<S>> {
    pub(super) inner: RevertableWriter<I>,
    pub(super) phantom: PhantomData<S>,
}

impl<S: Spec, I: StateProvider<S>> UniversalStateAccessor for TxScratchpad<S, I> {
    fn get(&mut self, namespace: Namespace, key: &SlotKey) -> (Option<SlotValue>, IsValueCached) {
        <RevertableWriter<I> as UniversalStateAccessor>::get(&mut self.inner, namespace, key)
    }

    fn set(&mut self, namespace: Namespace, key: &SlotKey, value: SlotValue) -> IsValueCached {
        <RevertableWriter<I> as UniversalStateAccessor>::set(&mut self.inner, namespace, key, value)
    }

    fn delete(&mut self, namespace: Namespace, key: &SlotKey) -> IsValueCached {
        <RevertableWriter<I> as UniversalStateAccessor>::delete(&mut self.inner, namespace, key)
    }
}

/// The list of changes caused by a single transaction
pub struct TxChangeSet {
    #[allow(missing_docs)]
    pub changes: Vec<((SlotKey, sov_state::Namespace), Option<SlotValue>)>,
}

impl<S: Spec, I: StateProvider<S>> TxScratchpad<S, I> {
    /// Commits the changes of this [`TxScratchpad`] and returns a [`StateCheckpoint`].
    pub fn commit(self) -> I {
        self.inner.commit()
    }

    /// Gets an iterator over the diff currently written onto this scratchpad. These changes will
    /// be reverted or commited as a unit.
    pub fn changes(&self) -> TxChangeSet {
        TxChangeSet {
            changes: self.inner.changes(),
        }
    }

    /// Reverts the changes of this [`TxScratchpad`] and returns a [`StateCheckpoint`].
    pub fn revert(self) -> I {
        self.inner.revert()
    }

    /// Converts this [`TxScratchpad`] into a [`PreExecWorkingSet`].
    pub fn to_pre_exec_working_set(
        self,
        gas_meter: BasicGasMeter<S::Gas>,
    ) -> PreExecWorkingSet<S, I> {
        PreExecWorkingSet {
            inner: self,
            gas_meter,
        }
    }
}

impl<S: Spec, I: StateProvider<S>> VersionReader for TxScratchpad<S, I> {
    fn rollup_height_to_access(&self) -> SlotNumber {
        self.inner.inner.rollup_height_to_access()
    }
}

/// A working set that can be used to charge gas for pre transaction execution checks.
pub struct PreExecWorkingSet<S: Spec, I: StateProvider<S>> {
    inner: TxScratchpad<S, I>,
    gas_meter: BasicGasMeter<S::Gas>,
}

impl<S: Spec, I: StateProvider<S>> PreExecWorkingSet<S, I> {
    /// Returns the associated gas meter and the scratchpad.
    pub fn to_scratchpad_and_gas_meter(self) -> (TxScratchpad<S, I>, BasicGasMeter<S::Gas>) {
        (self.inner, self.gas_meter)
    }
}

impl<S: Spec, I: StateProvider<S>> GasMeter<S::Gas> for PreExecWorkingSet<S, I> {
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

impl<S: Spec, I: StateProvider<S>> UniversalStateAccessor for PreExecWorkingSet<S, I> {
    fn get(&mut self, namespace: Namespace, key: &SlotKey) -> (Option<SlotValue>, IsValueCached) {
        <TxScratchpad<S, I> as UniversalStateAccessor>::get(&mut self.inner, namespace, key)
    }

    fn set(&mut self, namespace: Namespace, key: &SlotKey, value: SlotValue) -> IsValueCached {
        <TxScratchpad<S, I> as UniversalStateAccessor>::set(&mut self.inner, namespace, key, value)
    }

    fn delete(&mut self, namespace: Namespace, key: &SlotKey) -> IsValueCached {
        <TxScratchpad<S, I> as UniversalStateAccessor>::delete(&mut self.inner, namespace, key)
    }
}

impl<S: Spec, I: StateProvider<S>> VersionReader for PreExecWorkingSet<S, I> {
    fn rollup_height_to_access(&self) -> SlotNumber {
        self.inner.rollup_height_to_access()
    }
}

#[cfg(feature = "test-utils")]
impl<Store: crate::Storage> StateCheckpoint<Store> {
    /// Produces an unmetered [`WorkingSet`] from this [`StateProvider`].
    /// This is useful for tests that don't need to track gas consumption.
    pub fn to_working_set_unmetered<S: Spec<Storage = Store>>(self) -> WorkingSet<S, Self> {
        WorkingSet {
            delta: RevertableWriter::new(self.to_tx_scratchpad()),
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
pub struct WorkingSet<S: Spec, I: StateProvider<S> = StateCheckpoint<<S as Spec>::Storage>> {
    pub(super) delta: RevertableWriter<TxScratchpad<S, I>>,
    events: Vec<TypedEvent>,
    gas_meter: BasicGasMeter<S::Gas>,
    // Gas parameters of the transaction associated with the working set
    max_fee: u64,
    max_priority_fee_bips: PriorityFeeBips,
}

impl<S: Spec, I: StateProvider<S>> WorkingSet<S, I> {
    /// Creates a new [`WorkingSet`] from the provided [`TxScratchpad`] and [`AuthenticatedTransactionData`].
    pub fn create_working_set(
        scratchpad: TxScratchpad<S, I>,
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
    #[allow(clippy::type_complexity)]
    pub fn finalize(
        self,
    ) -> (
        TxScratchpad<S, I>,
        TransactionConsumption<S::Gas>,
        Vec<TypedEvent>,
    ) {
        let tx_reward = self.transaction_consumption();
        (self.delta.commit(), tx_reward, self.events)
    }

    /// Reverts the most recent changes to this [`WorkingSet`], returning a pristine
    /// [`TxScratchpad`] instance.
    pub fn revert(self) -> (TxScratchpad<S, I>, TransactionConsumption<S::Gas>) {
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

    /// Returns the maximum fee that can be paid for this transaction expressed in gas token amount.
    pub fn max_fee(&self) -> u64 {
        self.max_fee
    }
}

#[cfg(test)]
use crate::capabilities::Kernel;

#[cfg(test)]
impl<S: Spec> WorkingSet<S, StateCheckpoint<S::Storage>> {
    /// A helper function to create a new [`WorkingSet`] with a given gas price and remaining funds.
    /// Note: This method uses a [`MockKernel`] with a default height, this is not compatible with tests over multiple slots.
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
            phantom: PhantomData,
        };

        WorkingSet {
            delta: RevertableWriter::new(tx_scratchpad),
            events: Default::default(),
            gas_meter: BasicGasMeter::new(remaining_funds, price.clone()),
            max_fee: 0,
            max_priority_fee_bips: PriorityFeeBips::ZERO,
        }
    }

    /// Creates a new [`WorkingSet`] instance backed by the given [`Spec::Storage`] and a [`Kernel`].
    pub fn new_with_kernel<K: Kernel<S>>(inner: S::Storage, kernel: &K) -> Self {
        let state_checkpoint: StateCheckpoint<S::Storage> = StateCheckpoint::new(inner, kernel);
        let tx_scratchpad = TxScratchpad {
            inner: RevertableWriter::new(state_checkpoint),
            phantom: PhantomData,
        };

        WorkingSet {
            delta: RevertableWriter::new(tx_scratchpad),
            events: Default::default(),
            gas_meter: BasicGasMeter::new(u64::MAX, <S::Gas as crate::Gas>::Price::ZEROED),
            max_fee: 0,
            max_priority_fee_bips: PriorityFeeBips::ZERO,
        }
    }
}

impl<S: Spec, I: StateProvider<S>> GasMeter<S::Gas> for WorkingSet<S, I> {
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

impl<S: Spec, I: StateProvider<S>> UniversalStateAccessor for WorkingSet<S, I> {
    fn get(&mut self, namespace: Namespace, key: &SlotKey) -> (Option<SlotValue>, IsValueCached) {
        self.delta.get(namespace, key)
    }
    fn set(&mut self, namespace: Namespace, key: &SlotKey, value: SlotValue) -> IsValueCached {
        self.delta.set(namespace, key, value)
    }

    fn delete(&mut self, namespace: Namespace, key: &SlotKey) -> IsValueCached {
        self.delta.delete(namespace, key)
    }
}

impl<S: Spec, I: StateProvider<S>> EventContainer for WorkingSet<S, I> {
    fn add_event<E: 'static + core::marker::Send>(&mut self, event_key: &str, event: E) {
        self.events.push(TypedEvent::new(event_key, event));
    }
}

impl<S: Spec, I: StateProvider<S>> VersionReader for WorkingSet<S, I> {
    fn rollup_height_to_access(&self) -> SlotNumber {
        self.delta.inner.rollup_height_to_access()
    }
}

#[cfg(test)]
mod tests {
    use sov_rollup_interface::common::HexString;
    use sov_state::codec::BcsCodec;
    use sov_state::namespaces::User;
    use sov_state::{Kernel, SlotKey, SlotValue};
    use sov_test_utils::storage::SimpleStorageManager;
    use sov_test_utils::{MockDaSpec, MockZkvm};

    use crate::capabilities::mocks::MockKernel;
    use crate::capabilities::Kernel as _;
    use crate::execution_mode::Native;
    use crate::{Spec, StateCheckpoint, StateReader, StateWriter, WorkingSet};

    type TestSpec = crate::default_spec::DefaultSpec<MockDaSpec, MockZkvm, MockZkvm, Native>;

    #[test]
    fn test_workingset_get() {
        let codec = BcsCodec {};
        let storage_manager = SimpleStorageManager::new();
        let storage = storage_manager.create_storage();

        let prefix = sov_state::Prefix::new(vec![1, 2, 3]);
        let storage_key = SlotKey::new::<HexString, _, _>(&prefix, [4, 5, 6].as_ref(), &codec);
        let storage_value = SlotValue::new(&vec![7, 8, 9], &codec);

        let mut working_set = WorkingSet::<TestSpec>::new_with_kernel(
            storage.clone(),
            &MockKernel::<TestSpec>::default(),
        );
        StateWriter::<User>::set(&mut working_set, &storage_key, storage_value.clone()).expect("The set operation should succeed because there should be enough funds in the metered working set");
        let value = StateReader::<User>::get(&mut working_set, &storage_key).expect("The get operation should succeed because there should be enough funds in the metered working set");

        assert_eq!(Some(storage_value), value);
    }

    #[test]
    fn test_kernel_workingset_get() {
        let codec = BcsCodec {};
        let storage_manager = SimpleStorageManager::new();
        let storage = storage_manager.create_storage();

        let prefix = sov_state::Prefix::new(vec![1, 2, 3]);
        let storage_key = SlotKey::new::<HexString, _, _>(&prefix, [4, 5, 6].as_ref(), &codec);
        let storage_value = SlotValue::new(&vec![7, 8, 9], &codec);
        let kernel: MockKernel<TestSpec> = MockKernel::new(4, 1);

        let mut working_set =
            StateCheckpoint::<<TestSpec as Spec>::Storage>::new(storage.clone(), &kernel);
        let mut working_set = kernel.accessor(&mut working_set);

        StateWriter::<Kernel>::set(&mut working_set, &storage_key, storage_value.clone())
            .expect("This should be unfaillible");

        assert_eq!(
            Some(storage_value),
            StateReader::<Kernel>::get(&mut working_set, &storage_key)
                .expect("This should be unfaillible")
        );
    }
}
