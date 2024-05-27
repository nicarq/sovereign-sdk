//! Runtime state machine definitions.
use core::any::Any;
use core::fmt;
use std::boxed::Box;
use std::cmp::min;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};

use borsh::{BorshDeserialize, BorshSerialize};
pub use kernel_state::{KernelWorkingSet, VersionedStateReadWriter};
use sov_state::namespaces::User;
use sov_state::{
    namespaces, Accessory, CompileTimeNamespace, EventContainer, Namespace, ProvableStorageCache,
    SlotKey, SlotValue, StateAccesses, StateReader, StateReaderAndWriter, StateWriter, Storage,
};
#[cfg(feature = "native")]
use sov_state::{NativeStorage, ProvableCompileTimeNamespace, ProvenStateAccessor, StorageProof};

use crate::module::{Context, Spec};
use crate::transaction::{AuthenticatedTransactionData, PriorityFeeBips, TxGasMeter};
#[cfg(feature = "test-utils")]
use crate::UnlimitedGasMeter;
use crate::{Gas, GasArray, GasMeter, Genesis};

/// A helper trait allowing a type to access any namespace by their *runtime* enum variant.
pub(crate) trait UniversalStateAccessor {
    fn get(&mut self, namespace: Namespace, key: &SlotKey) -> Option<SlotValue>;
    fn set(&mut self, namespace: Namespace, key: &SlotKey, value: SlotValue);
    fn delete(&mut self, namespace: Namespace, key: &SlotKey);
}

/// A [`Delta`] is a diff over an underlying `Storage` instance. When queried, it first checks
/// whether the value is in its local cache and, if so, returns it. Otherwise, it queries the
/// underlying storage for the requested key, adds it to the Witness, and populates the value Into
/// its own local cache before returning.
///
/// Writes are always performed on the local cache, and are only committed to the underlying storage
/// when the `Delta` is frozen.
pub struct Delta<S: Storage> {
    inner: S,
    witness: S::Witness,
    kernel_cache: ProvableStorageCache<namespaces::Kernel>,
    user_cache: ProvableStorageCache<namespaces::User>,
    accessory_writes: HashMap<SlotKey, Option<SlotValue>>,
    version: Option<u64>,
}

impl<S: Storage> Delta<S> {
    fn new(inner: S, version: Option<u64>) -> Self {
        Self::with_witness(inner, Default::default(), version)
    }

    fn with_witness(inner: S, witness: S::Witness, version: Option<u64>) -> Self {
        Self {
            inner,
            witness,
            user_cache: Default::default(),
            kernel_cache: Default::default(),
            accessory_writes: Default::default(),
            version,
        }
    }

    fn freeze(self) -> (StateAccesses, AccessoryDelta<S>, S::Witness) {
        let Self {
            inner,
            user_cache,
            kernel_cache,
            accessory_writes,
            witness,
            version,
        } = self;

        (
            StateAccesses {
                user: user_cache.into(),
                kernel: kernel_cache.into(),
            },
            AccessoryDelta {
                version,
                writes: accessory_writes,
                storage: inner,
            },
            witness,
        )
    }
}

impl<S: Storage> UniversalStateAccessor for Delta<S> {
    fn get(&mut self, namespace: Namespace, key: &SlotKey) -> Option<SlotValue> {
        match namespace {
            Namespace::User => {
                self.user_cache
                    .get_or_fetch(key, &self.inner, &self.witness, self.version)
            }
            Namespace::Kernel => {
                self.kernel_cache
                    .get_or_fetch(key, &self.inner, &self.witness, self.version)
            }
            Namespace::Accessory => match self.accessory_writes.get(key).cloned() {
                Some(Some(value)) => Some(value),
                Some(None) => None,
                None => self.inner.get_accessory(key, self.version),
            },
        }
    }

    fn set(&mut self, namespace: Namespace, key: &SlotKey, value: SlotValue) {
        match namespace {
            Namespace::User => self.user_cache.set(key, value),
            Namespace::Kernel => self.kernel_cache.set(key, value),
            Namespace::Accessory => {
                self.accessory_writes.insert(key.clone(), Some(value));
            }
        }
    }

    fn delete(&mut self, namespace: Namespace, key: &SlotKey) {
        match namespace {
            Namespace::User => self.user_cache.delete(key),
            Namespace::Kernel => self.kernel_cache.delete(key),
            Namespace::Accessory => {
                self.accessory_writes.remove(key);
            }
        }
    }
}

impl<S: Storage> fmt::Debug for Delta<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Delta").finish()
    }
}

/// A delta containing *only* the accessory state.
pub struct AccessoryDelta<S: Storage> {
    // This inner storage is never accessed inside the zkVM because reads are
    // not allowed, so it can result as dead code.
    #[allow(dead_code)]
    version: Option<u64>,
    writes: HashMap<SlotKey, Option<SlotValue>>,
    storage: S,
}

impl<S: Storage> AccessoryDelta<S> {
    /// Freeze the accessory delta, preventing further accesses.
    pub fn freeze(self) -> Vec<(SlotKey, Option<SlotValue>)> {
        self.writes.into_iter().collect()
    }
}

impl<S: Storage> StateReader<Accessory> for AccessoryDelta<S> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        if let Some(value) = self.writes.get(key) {
            return value.clone().map(Into::into);
        }
        self.storage.get_accessory(key, self.version)
    }
}

impl<S: Storage> StateWriter<Accessory> for AccessoryDelta<S> {
    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        self.writes.insert(key.clone(), Some(value));
    }

    fn delete(&mut self, key: &SlotKey) {
        self.writes.insert(key.clone(), None);
    }
}

/// This structure is responsible for storing the `read-write` set.
///
/// A [`StateCheckpoint`] can be obtained from a [`WorkingSet`] in two ways:
///  1. With [`WorkingSet::checkpoint`].
///  2. With [`WorkingSet::revert`].
pub struct StateCheckpoint<S: Spec> {
    delta: Delta<S::Storage>,
}

impl<S: Spec> StateCheckpoint<S> {
    /// Creates a new [`StateCheckpoint`] instance without any changes, backed
    /// by the given [`Storage`].
    pub fn new(inner: S::Storage) -> Self {
        Self {
            delta: Delta::new(inner.clone(), None),
        }
    }

    /// Returns a handler for the accessory state (non-JMT state).
    ///
    /// You can use this method when calling getters and setters on accessory
    /// state containers, like AccessoryStateMap.
    pub fn accessory_state(&mut self) -> AccessoryStateCheckpoint<S> {
        AccessoryStateCheckpoint { checkpoint: self }
    }

    /// Returns a handler for the kernel state (priveleged jmt state)
    ///
    /// You can use this method when calling getters and setters on accessory
    /// state containers, like KernelStateMap.
    pub fn versioned_state(&mut self, context: &Context<S>) -> VersionedStateReadWriter<Self> {
        VersionedStateReadWriter {
            ws: self,
            slot_num: context.visible_slot_number(),
        }
    }

    /// Creates a new [`StateCheckpoint`] instance without any changes, backed
    /// by the given [`Storage`] and witness.
    pub fn with_witness(inner: S::Storage, witness: <S::Storage as Storage>::Witness) -> Self {
        Self {
            delta: Delta::with_witness(inner.clone(), witness, None),
        }
    }

    /// Produces an unmetered [`WorkingSet`] from this [`StateCheckpoint`].
    /// This is useful for tests that don't need to track gas consumption.
    #[cfg(feature = "test-utils")]
    pub fn to_working_set_unmetered(self) -> WorkingSet<S> {
        let stashed_working_set = TxScratchpad {
            delta: RevertableWriter::new(self.delta),
        };

        WorkingSet {
            delta: RevertableWriter::new(stashed_working_set),
            events: Default::default(),
            gas_meter: TxGasMeter::unmetered(),
            max_fee: 0,
            max_priority_fee_bips: PriorityFeeBips::ZERO,
        }
    }

    /// Produces a metered [`WorkingSet`] from this [`StateCheckpoint`].
    /// This is useful for tests that need to bypass pre-execution checks.
    #[cfg(feature = "test-utils")]
    pub fn to_working_set(
        self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
    ) -> WorkingSet<S> {
        let stashed_working_set = TxScratchpad {
            delta: RevertableWriter::new(self.delta),
        };

        WorkingSet {
            delta: RevertableWriter::new(stashed_working_set),
            events: Default::default(),
            gas_meter: tx.gas_meter(gas_price),
            max_fee: tx.max_fee,
            max_priority_fee_bips: tx.max_priority_fee_bips,
        }
    }

    /// Produces an unmetered [`WorkingSet`] from a [`StateCheckpoint`] for genesis.
    pub fn to_working_set_genesis<G: Genesis>(
        self,
        // This argument prevents this method from being called outside of genesis.
        _config: &G::Config,
    ) -> WorkingSet<S> {
        let stashed_working_set = TxScratchpad {
            delta: RevertableWriter::new(self.delta),
        };

        WorkingSet {
            delta: RevertableWriter::new(stashed_working_set),
            events: Default::default(),
            gas_meter: TxGasMeter::unmetered(),
            max_fee: 0,
            max_priority_fee_bips: PriorityFeeBips::ZERO,
        }
    }

    /// Transforms this [`StateCheckpoint`] into a [`PreExecWorkingSet`].
    /// This method takes a [`GasMeter`] as an argument, which is used to charge the gas for the pre-execution checks from the sequencer.
    pub fn to_tx_scratchpad(self) -> TxScratchpad<S> {
        TxScratchpad::<S> {
            delta: RevertableWriter::new(self.delta),
        }
    }

    /// Extracts ordered reads, writes, and witness from this [`StateCheckpoint`].
    ///
    /// You can then use these to call [`Storage::validate_and_materialize`] or some
    /// of the other related [`Storage`] methods. Note that this data is moved
    /// **out** of the [`StateCheckpoint`] i.e. it can't be extracted twice.
    pub fn freeze(
        self,
    ) -> (
        StateAccesses,
        AccessoryDelta<S::Storage>,
        <S::Storage as Storage>::Witness,
    ) {
        self.delta.freeze()
    }
}

impl<N, S: Spec> StateReader<N> for StateCheckpoint<S>
where
    N: CompileTimeNamespace,
{
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        self.delta.get(N::NAMESPACE, key)
    }
}

impl<N, S: Spec> StateWriter<N> for StateCheckpoint<S>
where
    N: CompileTimeNamespace,
{
    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        self.delta.set(N::NAMESPACE, key, value);
    }

    fn delete(&mut self, key: &SlotKey) {
        self.delta.delete(N::NAMESPACE, key);
    }
}

/// A state diff over the storage that contains all the changes related to transaction execution.
/// This structure is built from a [`StateCheckpoint`] and is used in the entire transaction lifecycle (from
/// pre-execution checks to post execution state updates).
pub struct TxScratchpad<S: Spec> {
    delta: RevertableWriter<Delta<S::Storage>>,
}

impl<S: Spec, Meter: GasMeter<S::Gas>> From<PreExecWorkingSet<S, Meter>> for TxScratchpad<S> {
    fn from(value: PreExecWorkingSet<S, Meter>) -> Self {
        value.inner
    }
}

impl<S: Spec> StateReader<User> for TxScratchpad<S> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        self.delta.get(User::NAMESPACE, key)
    }
}

impl<S: Spec> StateWriter<User> for TxScratchpad<S> {
    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        self.delta.set(User::NAMESPACE, key, value);
    }

    fn delete(&mut self, key: &SlotKey) {
        self.delta.delete(User::NAMESPACE, key);
    }
}

impl<S: Spec> TxScratchpad<S> {
    fn delta(&self) -> &Delta<S::Storage> {
        &self.delta.inner
    }

    pub fn commit(self) -> StateCheckpoint<S> {
        StateCheckpoint {
            delta: self.delta.commit(),
        }
    }

    pub fn revert(self) -> StateCheckpoint<S> {
        StateCheckpoint {
            delta: self.delta.revert(),
        }
    }

    pub fn to_pre_exec_working_set<Meter: GasMeter<S::Gas>>(
        self,
        gas_meter: Meter,
    ) -> PreExecWorkingSet<S, Meter> {
        PreExecWorkingSet {
            inner: self,
            gas_meter,
        }
    }

    /// Produces an unmetered [`PreExecWorkingSet`] from this [`StateCheckpoint`].
    /// This is useful for tests that don't need to track gas consumption.
    #[cfg(feature = "test-utils")]
    pub fn pre_exec_ws_unmetered(self) -> PreExecWorkingSet<S, UnlimitedGasMeter<S::Gas>> {
        PreExecWorkingSet {
            inner: self,
            gas_meter: UnlimitedGasMeter::new(),
        }
    }

    /// Produces an unmetered [`PreExecWorkingSet`] from this [`StateCheckpoint`] for a given price.
    /// This is useful for tests that don't need to test failure over gas exhaustion.
    #[cfg(feature = "test-utils")]
    pub fn pre_exec_ws_unmetered_with_price(
        self,
        gas_price: &<S::Gas as Gas>::Price,
    ) -> PreExecWorkingSet<S, UnlimitedGasMeter<S::Gas>> {
        PreExecWorkingSet {
            inner: self,
            gas_meter: UnlimitedGasMeter::new_with_price(gas_price.clone()),
        }
    }
}

impl<S: Spec> UniversalStateAccessor for TxScratchpad<S> {
    fn get(&mut self, namespace: Namespace, key: &SlotKey) -> Option<SlotValue> {
        self.delta.get(namespace, key)
    }

    fn set(&mut self, namespace: Namespace, key: &SlotKey, value: SlotValue) {
        self.delta.set(namespace, key, value);
    }

    fn delete(&mut self, namespace: Namespace, key: &SlotKey) {
        self.delta.delete(namespace, key);
    }
}

/// A working set that can be used to charge gas for pre transaction execution checks.
pub struct PreExecWorkingSet<S: Spec, PreExecChecksMeter: GasMeter<S::Gas>> {
    inner: TxScratchpad<S>,
    gas_meter: PreExecChecksMeter,
}

pub struct AuthorizeTransactionError<S: Spec, PreExecChecksMeter: GasMeter<S::Gas>> {
    pub reason: anyhow::Error,
    pub pre_exec_working_set: PreExecWorkingSet<S, PreExecChecksMeter>,
}

impl<S: Spec, PreExecChecksMeter: GasMeter<S::Gas>> PreExecWorkingSet<S, PreExecChecksMeter> {
    /// Builds a [`WorkingSet`] from the this [`PreExecWorkingSet`].
    /// This method can fail if the transaction has not locked enough gas for the pre-execution checks.
    pub fn transfer_gas_to_working_set(
        self,
        tx: &AuthenticatedTransactionData<S>,
    ) -> Result<WorkingSet<S>, AuthorizeTransactionError<S, PreExecChecksMeter>> {
        let max_fee = tx.max_fee;

        let mut gas_meter = tx.gas_meter(self.gas_meter.gas_price());

        if let Err(e) = gas_meter.charge_gas(self.gas_meter.gas_used()) {
            return Err(AuthorizeTransactionError {
                reason: e,
                pre_exec_working_set: self,
            });
        }

        Ok(WorkingSet {
            delta: RevertableWriter::new(self.inner),
            events: Default::default(),
            gas_meter,
            max_fee,
            max_priority_fee_bips: tx.max_priority_fee_bips,
        })
    }
}

impl<S: Spec, Meter: GasMeter<S::Gas>> GasMeter<S::Gas> for PreExecWorkingSet<S, Meter> {
    fn gas_used(&self) -> &S::Gas {
        self.gas_meter.gas_used()
    }

    fn gas_price(&self) -> &<S::Gas as Gas>::Price {
        self.gas_meter.gas_price()
    }

    fn charge_gas(&mut self, amount: &S::Gas) -> anyhow::Result<(), anyhow::Error> {
        self.gas_meter.charge_gas(amount)
    }

    fn remaining_funds(&self) -> u64 {
        self.gas_meter.remaining_funds()
    }
}

impl<S: Spec, PreExecChecksMeter: GasMeter<S::Gas>> StateReader<User>
    for PreExecWorkingSet<S, PreExecChecksMeter>
{
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        UniversalStateAccessor::get(&mut self.inner, User::NAMESPACE, key)
    }
}

impl<S: Spec, PreExecChecksMeter: GasMeter<S::Gas>> StateWriter<User>
    for PreExecWorkingSet<S, PreExecChecksMeter>
{
    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        UniversalStateAccessor::set(&mut self.inner, User::NAMESPACE, key, value);
    }

    fn delete(&mut self, key: &SlotKey) {
        UniversalStateAccessor::delete(&mut self.inner, User::NAMESPACE, key);
    }
}

/// Represents a convenience struct to track the event and its type, functioning similarly to a typemap.
///
/// This struct is used to store information about an event, including its key, type identifier,
/// and the event itself encapsulated in a boxed trait object.
///
/// # Fields
/// - `event_key`: A vector of bytes representinexamples/simple-nft-module/README.mdg the unique key of the event.
/// - `type_id`: The type identifier of the event, using [`core::any::TypeId`].
/// - `boxed_event`: The event encapsulated in a box, implementing [`core::any::Any`] and [`core::marker::Send`].
#[derive(Debug)]
pub struct TypedEvent {
    event_key: Vec<u8>,
    type_id: core::any::TypeId,
    boxed_event: Box<dyn core::any::Any + core::marker::Send>,
}

impl TypedEvent {
    /// Created a Typed Event
    pub fn new<E: 'static + core::marker::Send>(event_key: &str, event: E) -> Self {
        TypedEvent {
            event_key: event_key.as_bytes().to_vec(),
            type_id: event.type_id(),
            boxed_event: Box::new(event),
        }
    }

    /// Try to cast from the TypedEvent to a specific type E provided
    /// checks type_id to avoid un-necessary casting
    pub fn downcast<E: core::clone::Clone + 'static>(self) -> Option<E> {
        if core::any::TypeId::of::<E>() == self.type_id {
            self.boxed_event.downcast::<E>().ok().map(|boxed| *boxed)
        } else {
            None
        }
    }

    /// Function to peek at the type id
    pub fn type_id(&self) -> &core::any::TypeId {
        &self.type_id
    }

    /// Function to peek at the event key
    pub fn event_key(&self) -> &[u8] {
        &self.event_key
    }
}

/// The format of the resources consumed by the transaction. The base fee and the priority fee are expressed as gas token amounts.
/// The [`TransactionConsumption`] data structure can only be built from the [`WorkingSet`] data structure.
///
/// ## Type safety
/// To build this data structure outside of `sov-modules-api`, one would need to call [`WorkingSet::finalize`] or [`WorkingSet::checkpoint`]
#[derive(PartialEq, Eq, Debug)]
pub struct TransactionConsumption<GU: Gas> {
    /// The amount of funds locked in the transaction that remains after transaction is executed and tip is processed.
    /// This amount includes the `base_fee` and the `priority_fee` gas token consumption
    pub(crate) remaining_funds: u64,
    /// The base fee reward of the transaction expressed as a gas token amount.
    pub(crate) base_fee: GU,
    /// The priority fee reward of the transaction expressed as a gas token amount.
    pub(crate) priority_fee: u64,
    /// The gas price of the transaction.
    pub(crate) gas_price: GU::Price,
}

impl<GU: Gas> TransactionConsumption<GU> {
    /// A zero consumption. Happens when the transaction is ignored (like in the case of a revert for the speculative execution mode).
    pub const ZERO: Self = Self {
        remaining_funds: 0,
        base_fee: GU::ZEROED,
        priority_fee: 0,
        gas_price: GU::Price::ZEROED,
    };

    /// The base fee reward of the transaction expressed as a gas token amount.
    pub const fn base_fee(&self) -> &GU {
        &self.base_fee
    }

    pub fn base_fee_value(&self) -> u64 {
        self.base_fee.value(&self.gas_price)
    }

    /// The priority fee reward of the transaction expressed as a gas token amount.
    pub const fn priority_fee(&self) -> u64 {
        self.priority_fee
    }

    /// If the total consumption overflows, we saturate, because we know that this amount will always be lower than the max fee.
    pub fn total_consumption(&self) -> u64 {
        self.base_fee
            .value(&self.gas_price)
            .saturating_add(self.priority_fee)
    }

    pub fn remaining_funds(&self) -> u64 {
        self.remaining_funds
    }
}

impl<GU: Gas> Display for TransactionConsumption<GU> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "TransactionConsumption {{ remaining_funds: {}, base_fee: {}, priority_fee: {}, gas_price: {} }}",
            self.remaining_funds, self.base_fee, self.priority_fee, self.gas_price
        )
    }
}

/// The type used to represent the sequencer reward. This type should be obtained from the [`TransactionConsumption`] type.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    BorshSerialize,
    BorshDeserialize,
)]
pub struct SequencerReward(u64);

impl SequencerReward {
    /// Returns a zero sequencer reward. This can be used to initialize an accumulator to build a sequencer reward.
    pub const ZERO: Self = Self(0);

    /// Adds another reward to this reward. Consumes the other reward.
    /// If the result overflows, we saturate.
    pub fn accumulate(&mut self, other: Self) {
        self.0 = self.0.saturating_add(other.0);
    }
}

impl<GU: Gas> From<TransactionConsumption<GU>> for SequencerReward {
    fn from(value: TransactionConsumption<GU>) -> Self {
        Self(value.priority_fee())
    }
}

impl From<SequencerReward> for u64 {
    fn from(val: SequencerReward) -> Self {
        val.0
    }
}

impl Display for SequencerReward {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SequencerReward({})", self.0)
    }
}

/// This structure contains the read-write set and the events collected during the execution of a transaction.
/// There are two ways to convert it into a StateCheckpoint:
/// 1. By using the [`WorkingSet::finalize`] method, where all the changes are added to the underlying
/// [`TxScratchpad`].
/// 2. By using the [`WorkingSet::revert`] method, where the most recent changes are reverted and the previous [`TxScratchpad`] is returned.
pub struct WorkingSet<S: Spec> {
    delta: RevertableWriter<TxScratchpad<S>>,
    events: Vec<TypedEvent>,
    gas_meter: TxGasMeter<S::Gas>,

    // Gas parameters of the transaction associated with the working set
    max_fee: u64,
    max_priority_fee_bips: PriorityFeeBips,
}

fn transaction_consumption_helper<S: Spec>(
    base_fee: &S::Gas,
    gas_price: &<S::Gas as Gas>::Price,
    max_fee: u64,
    max_priority_fee_bips: PriorityFeeBips,
) -> TransactionConsumption<S::Gas> {
    let base_fee_value = base_fee.value(gas_price);

    // We compute the `max_priority_fee_bips` by applying the `priority_fee_per_gas` to the consumed gas.
    let max_priority_fee_bips = max_priority_fee_bips
        .apply(base_fee_value)
        // if the computation overflows, we return the max fee - we always have `priority_fee <= tx.max_priority_fee_bips() <= tx.max_fee()`
        .unwrap_or(max_fee);

    // The tip is the minimum of the remaining gas allocated to the transaction and the maximum priority fee per gas.
    // We transfer the tip to the tip recipient address.
    let tip = min(max_priority_fee_bips, max_fee - base_fee_value);

    // Since the tip is an amount of gas tokens consumed on top of the base fee from the gas meter, we need to take that into
    // account in the calculation.
    let remaining_funds_including_tip = max_fee.saturating_sub(base_fee_value).saturating_sub(tip);

    TransactionConsumption {
        remaining_funds: remaining_funds_including_tip,
        base_fee: base_fee.clone(),
        priority_fee: tip,
        gas_price: gas_price.clone(),
    }
}

impl<S: Spec> WorkingSet<S> {
    /// Builds a [`TransactionConsumption`] from the [`WorkingSet`].
    pub(crate) fn transaction_consumption(&self) -> TransactionConsumption<S::Gas> {
        // The base fee is the amount of gas consumed by the transaction execution.
        let base_fee = self.gas_meter.gas_used();
        let gas_price = self.gas_meter.gas_price();

        transaction_consumption_helper::<S>(
            base_fee,
            gas_price,
            self.max_fee,
            self.max_priority_fee_bips,
        )
    }

    /// Creates a new [`WorkingSet`] instance backed by the given [`Storage`].
    ///
    /// The witness value is set to [`Default::default`]. Use
    /// [`WorkingSet::with_witness`] to set a custom witness value.
    ///
    /// ## TODO(@theochap)
    /// `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/678>`: This method is *deprecated* and should be removed once we have a way to call rpc methods using the [`StateCheckpoint`].
    pub fn new(inner: S::Storage) -> Self {
        let state_checkpoint: StateCheckpoint<S> = StateCheckpoint::new(inner);
        let tx_scratchpad = TxScratchpad {
            delta: RevertableWriter::new(state_checkpoint.delta),
        };

        WorkingSet {
            delta: RevertableWriter::new(tx_scratchpad),
            events: Default::default(),
            gas_meter: TxGasMeter::unmetered(),
            max_fee: 0,
            max_priority_fee_bips: PriorityFeeBips::ZERO,
        }
    }

    /// Creates a new archival working set with the same underlying `Storage` but an empty Delta, without
    /// modifying the original [`WorkingSet`].
    /// Propagates the gas meter to the new working set.
    ///
    /// ## TODO(@theochap)
    /// `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/678>`: This method is *deprecated* should be removed once we have a way to call rpc methods using the [`StateCheckpoint`].
    pub fn get_archival_at(&self, version: u64) -> Self {
        let storage = self.storage().clone();
        let tx_scratchpad = TxScratchpad {
            delta: RevertableWriter::new(Delta::new(storage.clone(), Some(version))),
        };

        Self {
            delta: RevertableWriter::new(tx_scratchpad),
            events: Default::default(),
            gas_meter: TxGasMeter::unmetered(),
            max_fee: 0,
            max_priority_fee_bips: PriorityFeeBips::ZERO,
        }
    }

    /// Returns a handler for the kernel state (priveleged jmt state)
    ///
    /// You can use this method when calling getters and setters on accessory
    /// state containers, like KernelStateMap.
    pub fn versioned_state(&mut self, context: &Context<S>) -> VersionedStateReadWriter<Self> {
        VersionedStateReadWriter {
            ws: self,
            slot_num: context.visible_slot_number(),
        }
    }

    /// Returns a handler for the kernel state for genesis
    ///
    /// You can use this method when calling getters and setters on accessory
    /// state containers, like KernelStateMap.
    pub fn genesis_versioned_state(&mut self) -> VersionedStateReadWriter<Self> {
        VersionedStateReadWriter {
            ws: self,
            slot_num: 0,
        }
    }

    /// Creates a new [`WorkingSet`] instance backed by the given [`Storage`]
    /// and a custom witness value.
    ///
    /// ## TODO(@theochap)
    /// This method is *deprecated* and should be removed once we have completed the gas integration for state accesses.
    pub fn with_witness(inner: S::Storage, witness: <S::Storage as Storage>::Witness) -> Self {
        let state_checkpoint: StateCheckpoint<S> = StateCheckpoint::with_witness(inner, witness);
        let tx_scratchpad = TxScratchpad {
            delta: RevertableWriter::new(state_checkpoint.delta),
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
    /// committing the changes to the underlying [`Storage`] via
    /// [`StateCheckpoint::freeze`].
    ///
    /// ## Safety note
    /// This function calls [`WorkingSet::finalize`] under the hood, please be sure that we can skip this
    /// intermediary committing step. This function is only accessible in tests
    #[cfg(feature = "test-utils")]
    pub fn checkpoint(
        self,
    ) -> (
        StateCheckpoint<S>,
        TransactionConsumption<S::Gas>,
        Vec<TypedEvent>,
    ) {
        let (tx_scratchpad, transaction_consumption, events) = self.finalize();
        let checkpoint = tx_scratchpad.commit();

        (checkpoint, transaction_consumption, events)
    }

    /// Turns this [`WorkingSet`] into a [`TxScratchpad`], commits the changes to the [`WorkingSet`] to the
    /// inner scratchpad.
    pub fn finalize(
        self,
    ) -> (
        TxScratchpad<S>,
        TransactionConsumption<S::Gas>,
        Vec<TypedEvent>,
    ) {
        let tx_reward = self.transaction_consumption();
        (self.delta.commit(), tx_reward, self.events)
    }

    /// Reverts the most recent changes to this [`WorkingSet`], returning a pristine
    /// [`TxScratchpad`] instance.
    pub fn revert(self) -> (TxScratchpad<S>, TransactionConsumption<S::Gas>) {
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
        self.gas_meter.remaining_funds()
    }

    fn inner(&self) -> &TxScratchpad<S> {
        &self.delta.inner
    }

    fn storage(&self) -> &S::Storage {
        &self.inner().delta().inner
    }

    #[cfg(feature = "native")]
    fn version(&self) -> Option<u64> {
        self.inner().delta().version
    }

    /// Returns the maximum fee that can be paid for this transaction expressed in gas token amount.
    pub fn max_fee(&self) -> u64 {
        self.max_fee
    }
}

impl<S: Spec> EventContainer for WorkingSet<S> {
    fn add_event<E: 'static + core::marker::Send>(&mut self, event_key: &str, event: E) {
        self.events.push(TypedEvent::new(event_key, event));
    }
}

impl<S: Spec> GasMeter<S::Gas> for WorkingSet<S> {
    fn charge_gas(&mut self, gas: &S::Gas) -> anyhow::Result<()> {
        self.gas_meter.charge_gas(gas)
    }

    fn gas_price(&self) -> &<S::Gas as Gas>::Price {
        self.gas_meter.gas_price()
    }

    fn gas_used(&self) -> &S::Gas {
        self.gas_meter.gas_used()
    }

    fn remaining_funds(&self) -> u64 {
        self.gas_meter.remaining_funds()
    }
}

#[cfg(feature = "native")]
impl<N: ProvableCompileTimeNamespace, S: Spec> ProvenStateAccessor<N> for WorkingSet<S>
where
    WorkingSet<S>: StateReaderAndWriter<N>,
{
    type Proof = <S::Storage as Storage>::Proof;

    fn get_with_proof(&mut self, key: SlotKey) -> StorageProof<Self::Proof> {
        self.storage().get_with_proof::<N>(key, self.version())
    }
}
impl<S: Spec> StateReader<User> for WorkingSet<S> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        self.delta.get(User::NAMESPACE, key)
    }
}

impl<S: Spec> StateWriter<User> for WorkingSet<S> {
    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        self.delta.set(User::NAMESPACE, key, value);
    }

    fn delete(&mut self, key: &SlotKey) {
        self.delta.delete(User::NAMESPACE, key);
    }
}

impl<S: Spec> StateReader<Accessory> for WorkingSet<S> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        if !cfg!(feature = "native") {
            None
        } else {
            self.delta.get(Accessory::NAMESPACE, key)
        }
    }
}

impl<S: Spec> StateWriter<Accessory> for WorkingSet<S> {
    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        self.delta.set(Accessory::NAMESPACE, key, value);
    }

    fn delete(&mut self, key: &SlotKey) {
        self.delta.delete(Accessory::NAMESPACE, key);
    }
}

/// A wrapper over [`WorkingSet`] that only allows access to the accessory
/// state (non-JMT state).
pub struct AccessoryStateCheckpoint<'a, S: Spec> {
    checkpoint: &'a mut StateCheckpoint<S>,
}

impl<'a, S: Spec> StateReader<Accessory> for AccessoryStateCheckpoint<'a, S> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        if !cfg!(feature = "native") {
            None
        } else {
            self.checkpoint.delta.get(Accessory::NAMESPACE, key)
        }
    }
}

impl<'a, S: Spec> StateWriter<Accessory> for AccessoryStateCheckpoint<'a, S> {
    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        self.checkpoint.delta.set(Accessory::NAMESPACE, key, value);
    }

    fn delete(&mut self, key: &SlotKey) {
        self.checkpoint.delta.delete(Accessory::NAMESPACE, key);
    }
}

/// Provides specialized working set wrappers for dealing with protected state.
pub mod kernel_state {
    use sov_rollup_interface::da::DaSpec;
    use sov_state::namespaces;

    use super::*;
    use crate::capabilities::Kernel;

    /// A trait indicating that this working set is version aware
    pub trait VersionReader: StateReaderAndWriter<namespaces::Kernel> {
        /// Returns the current version of the working set
        fn current_version(&self) -> u64;
    }

    impl<'a, S: Spec> VersionReader for VersionedStateReadWriter<'a, StateCheckpoint<S>> {
        fn current_version(&self) -> u64 {
            self.slot_num
        }
    }

    /// A wrapper over [`WorkingSet`] that allows access to kernel values
    pub struct VersionedStateReadWriter<'a, S> {
        pub(super) ws: &'a mut S,
        pub(super) slot_num: u64,
    }

    impl<'a, S: Spec> VersionedStateReadWriter<'a, StateCheckpoint<S>> {
        /// Instantiates a [`VersionedStateReadWriter`] from a kernel working set.
        /// Sets the `slot_num` to the virtual slot number of the kernel.
        pub fn from_kernel_ws_virtual(
            kernel_ws: KernelWorkingSet<'a, S>,
        ) -> VersionedStateReadWriter<'a, StateCheckpoint<S>> {
            VersionedStateReadWriter {
                ws: kernel_ws.inner,
                slot_num: kernel_ws.virtual_slot_num,
            }
        }
    }

    impl<'a, S> VersionedStateReadWriter<'a, S> {
        /// Returns the working slot number
        pub fn slot_num(&self) -> u64 {
            self.slot_num
        }

        /// Returns a reference to the inner working set
        pub fn get_ws(&self) -> &S {
            self.ws
        }

        /// Returns a mutable reference to the inner working set
        pub fn get_ws_mut(&mut self) -> &mut S {
            self.ws
        }
    }

    impl<'a, S: Spec> StateReader<namespaces::Kernel>
        for VersionedStateReadWriter<'a, StateCheckpoint<S>>
    {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            self.ws.delta.get(namespaces::Kernel::NAMESPACE, key)
        }
    }

    impl<'a, S: Spec> StateWriter<namespaces::Kernel>
        for VersionedStateReadWriter<'a, StateCheckpoint<S>>
    {
        fn set(&mut self, key: &SlotKey, value: SlotValue) {
            self.ws.delta.set(namespaces::Kernel::NAMESPACE, key, value);
        }

        fn delete(&mut self, key: &SlotKey) {
            self.ws.delta.delete(namespaces::Kernel::NAMESPACE, key);
        }
    }

    impl<'a, S: Spec> StateReader<namespaces::Kernel> for VersionedStateReadWriter<'a, WorkingSet<S>> {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            self.ws.delta.get(namespaces::Kernel::NAMESPACE, key)
        }
    }

    impl<'a, S: Spec> StateWriter<namespaces::Kernel> for VersionedStateReadWriter<'a, WorkingSet<S>> {
        fn set(&mut self, key: &SlotKey, value: SlotValue) {
            self.ws.delta.set(namespaces::Kernel::NAMESPACE, key, value);
        }

        fn delete(&mut self, key: &SlotKey) {
            self.ws.delta.delete(namespaces::Kernel::NAMESPACE, key);
        }
    }

    /// A special wrapper over [`WorkingSet`] that allows access to kernel values to bootstrap the kernel working set
    pub struct BootstrapWorkingSet<'a, S: Spec> {
        /// The inner working set
        pub(crate) inner: &'a mut StateCheckpoint<S>,
    }

    impl<'a, S: Spec> StateReader<User> for BootstrapWorkingSet<'a, S> {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            self.inner.delta.get(User::NAMESPACE, key)
        }
    }

    impl<'a, S: Spec> StateWriter<User> for BootstrapWorkingSet<'a, S> {
        fn set(&mut self, key: &SlotKey, value: SlotValue) {
            self.inner.delta.set(User::NAMESPACE, key, value);
        }

        fn delete(&mut self, key: &SlotKey) {
            self.inner.delta.delete(User::NAMESPACE, key);
        }
    }

    impl<'a, S: Spec> StateReader<namespaces::Kernel> for BootstrapWorkingSet<'a, S> {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            self.inner.delta.get(namespaces::Kernel::NAMESPACE, key)
        }
    }

    impl<'a, S: Spec> StateWriter<namespaces::Kernel> for BootstrapWorkingSet<'a, S> {
        fn set(&mut self, key: &SlotKey, value: SlotValue) {
            self.inner
                .delta
                .set(namespaces::Kernel::NAMESPACE, key, value);
        }

        fn delete(&mut self, key: &SlotKey) {
            self.inner.delta.delete(namespaces::Kernel::NAMESPACE, key);
        }
    }

    /// A wrapper over [`WorkingSet`] that allows access to kernel values
    pub struct KernelWorkingSet<'a, S: Spec> {
        /// The inner working set
        pub inner: &'a mut StateCheckpoint<S>,
        /// The actual current slot number
        pub(super) true_slot_num: u64,
        /// The slot number visible to user-space modules
        pub(super) virtual_slot_num: u64,
    }

    impl<'a, S: Spec> VersionReader for KernelWorkingSet<'a, S> {
        fn current_version(&self) -> u64 {
            self.true_slot_num
        }
    }

    impl<'a, S: Spec> KernelWorkingSet<'a, S> {
        /// This private method instantiates a bootstrap working set to initialize a kernel
        fn get_bootstrap(inner: &'a mut StateCheckpoint<S>) -> BootstrapWorkingSet<'a, S> {
            BootstrapWorkingSet { inner }
        }

        /// Build a new kernel working set from the associated kernel
        pub fn from_kernel<K: Kernel<S, Da>, Da: DaSpec>(
            kernel: &K,
            ws: &'a mut StateCheckpoint<S>,
        ) -> Self {
            let mut bootstrapper = KernelWorkingSet::get_bootstrap(ws);
            let true_slot_num = kernel.true_slot_number(&mut bootstrapper);
            let virtual_slot_num = kernel.visible_slot_number(&mut bootstrapper);
            Self {
                inner: ws,
                true_slot_num,
                virtual_slot_num,
            }
        }

        /// Returns a kernel working set with its heights intiialized to 0.
        /// This is intended to be used for genesis setup only.
        pub fn uninitialized(ws: &'a mut StateCheckpoint<S>) -> Self {
            Self {
                inner: ws,
                true_slot_num: 0,
                virtual_slot_num: 0,
            }
        }

        /// Returns the true slot number
        pub fn current_slot(&self) -> u64 {
            self.true_slot_num
        }

        /// Returns the slot number visible from user space
        pub fn virtual_slot(&self) -> u64 {
            self.virtual_slot_num
        }

        /// Updates the kernel working set internals
        pub fn update_true_slot_number(&mut self, true_slot_num: u64) {
            self.true_slot_num = true_slot_num;
        }

        /// Updates the kernel working set internals
        pub fn update_virtual_height(&mut self, virtual_height: u64) {
            self.virtual_slot_num = virtual_height;
        }
    }

    impl<'a, S: Spec> StateReader<User> for KernelWorkingSet<'a, S> {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            self.inner.delta.get(User::NAMESPACE, key)
        }
    }

    impl<'a, S: Spec> StateWriter<User> for KernelWorkingSet<'a, S> {
        fn set(&mut self, key: &SlotKey, value: SlotValue) {
            self.inner.delta.set(User::NAMESPACE, key, value);
        }

        fn delete(&mut self, key: &SlotKey) {
            self.inner.delta.delete(User::NAMESPACE, key);
        }
    }
    impl<'a, S: Spec> StateReader<namespaces::Kernel> for KernelWorkingSet<'a, S> {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            self.inner.delta.get(namespaces::Kernel::NAMESPACE, key)
        }
    }

    impl<'a, S: Spec> StateWriter<namespaces::Kernel> for KernelWorkingSet<'a, S> {
        fn set(&mut self, key: &SlotKey, value: SlotValue) {
            self.inner
                .delta
                .set(namespaces::Kernel::NAMESPACE, key, value);
        }

        fn delete(&mut self, key: &SlotKey) {
            self.inner.delta.delete(namespaces::Kernel::NAMESPACE, key);
        }
    }
}

struct RevertableWriter<T> {
    inner: T,
    writes: HashMap<(SlotKey, Namespace), Option<SlotValue>>,
}

impl<T: fmt::Debug> fmt::Debug for RevertableWriter<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RevertableWriter")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<T> RevertableWriter<T> {
    fn new(inner: T) -> Self {
        Self {
            inner,
            writes: Default::default(),
        }
    }

    /// Commit all items from `RevertableWriter` returning the inner storage.
    fn commit(mut self) -> T
    where
        T: UniversalStateAccessor,
    {
        for ((key, namespace), value) in self.writes.into_iter() {
            Self::commit_entry(&mut self.inner, namespace, key, value);
        }

        self.inner
    }

    fn revert(self) -> T {
        self.inner
    }

    fn commit_entry(inner: &mut T, namespace: Namespace, key: SlotKey, value: Option<SlotValue>)
    where
        T: UniversalStateAccessor,
    {
        match value {
            Some(value) => inner.set(namespace, &key, value),
            None => inner.delete(namespace, &key),
        }
    }
}

impl<T> UniversalStateAccessor for RevertableWriter<T>
where
    T: UniversalStateAccessor,
{
    fn get(&mut self, namespace: Namespace, key: &SlotKey) -> Option<SlotValue> {
        if let Some(value) = self.writes.get(&(key.clone(), namespace)) {
            value.as_ref().cloned().map(Into::into)
        } else {
            self.inner.get(namespace, key)
        }
    }

    fn set(&mut self, namespace: Namespace, key: &SlotKey, value: SlotValue) {
        self.writes.insert((key.clone(), namespace), Some(value));
    }

    fn delete(&mut self, namespace: Namespace, key: &SlotKey) {
        self.writes.insert((key.clone(), namespace), None);
    }
}

#[cfg(test)]
mod tests {
    use sov_mock_zkvm::MockZkVerifier;
    use sov_rollup_interface::execution_mode::Native;

    use super::{PriorityFeeBips, SequencerReward, TransactionConsumption};
    use crate::default_spec::DefaultSpec;
    use crate::scratchpad::transaction_consumption_helper;
    use crate::{GasArray, GasPrice, GasUnit};

    /// Consume all the remaining gas, so the transaction reward is the same as the base fee and there is no priority fee.
    #[test]
    fn test_compute_transaction_reward_consume_all_gas() {
        const REMAINING_FUNDS: u64 = 100;

        let tx_reward =
            transaction_consumption_helper::<DefaultSpec<MockZkVerifier, MockZkVerifier, Native>>(
                &GasArray::from_slice(&[REMAINING_FUNDS / 2; 2]),
                &GasPrice::from_slice(&[1; 2]),
                REMAINING_FUNDS,
                PriorityFeeBips::from_percentage(10),
            );

        assert_eq!(
            tx_reward,
            TransactionConsumption {
                remaining_funds: 0,
                base_fee: GasArray::from_slice(&[REMAINING_FUNDS / 2; 2]),
                priority_fee: 0,
                gas_price: GasPrice::from_slice(&[1; 2])
            }
        );
    }

    /// Consume half of the remaining gas, so the transaction reward is half of the initial funds and there is a maximum priority fee (100%).
    #[test]
    fn test_compute_transaction_reward_consume_not_all_gas() {
        const REMAINING_FUNDS: u64 = 100;

        let tx_reward =
            transaction_consumption_helper::<DefaultSpec<MockZkVerifier, MockZkVerifier, Native>>(
                &GasArray::from_slice(&[REMAINING_FUNDS / 4; 2]),
                &GasPrice::from_slice(&[1; 2]),
                REMAINING_FUNDS,
                PriorityFeeBips::from_percentage(100),
            );

        assert_eq!(
            tx_reward,
            TransactionConsumption {
                remaining_funds: 0,
                base_fee: GasArray::from_slice(&[REMAINING_FUNDS / 4; 2]),
                priority_fee: 50,
                gas_price: GasPrice::from_slice(&[1; 2])
            }
        );
    }

    #[test]
    fn test_display_transaction_reward() {
        let tx_reward = TransactionConsumption::<GasUnit<2>> {
            remaining_funds: 10,
            base_fee: GasUnit::from_slice(&[100; 2]),
            priority_fee: 50,
            gas_price: GasPrice::from_slice(&[1; 2]),
        };

        assert_eq!(
            format!("{}", tx_reward),
            "TransactionConsumption { remaining_funds: 10, base_fee: GasUnit[100, 100], priority_fee: 50, gas_price: GasPrice[1, 1] }"
        );
    }

    #[test]
    fn test_display_sequencer_reward() {
        let seq_reward = SequencerReward(100);

        assert_eq!(format!("{}", seq_reward), "SequencerReward(100)");
    }
}
