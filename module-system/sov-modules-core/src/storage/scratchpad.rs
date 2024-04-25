//! Runtime state machine definitions.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::Any;
use core::fmt;

pub use kernel_state::{KernelWorkingSet, VersionedStateReadWriter};
use sov_rollup_interface::maybestd::collections::HashMap;

use crate::common::GasMeter;
use crate::module::{Context, Spec};
use crate::namespaces::{self, Accessory, CompileTimeNamespace, User};
#[cfg(feature = "native")]
use crate::storage::{NativeStorage, ProvableCompileTimeNamespace, StorageProof};
use crate::storage::{
    ProvableStorageCache, SlotKey, SlotValue, StateCodec, StateItemCodec, StateItemDecoder, Storage,
};
use crate::{Gas, Namespace, StateAccesses};

/// A storage reader and writer which can access a particular namespace.
pub trait StateReaderAndWriter<N: CompileTimeNamespace> {
    /// Get a value from the storage.
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue>;

    /// Replaces a storage value.
    fn set(&mut self, key: &SlotKey, value: SlotValue);

    /// Deletes a storage value.
    fn delete(&mut self, key: &SlotKey);

    /// Removes a storage value
    fn remove(&mut self, key: &SlotKey) -> Option<SlotValue> {
        let value = self.get(key);
        self.delete(key);
        value
    }

    /// Get a decoded value from the storage.
    fn get_decoded<V, Codec>(&mut self, storage_key: &SlotKey, codec: &Codec) -> Option<V>
    where
        Codec: StateCodec,
        Codec::ValueCodec: StateItemCodec<V>,
    {
        let storage_value = self.get(storage_key)?;

        Some(codec.value_codec().decode_unwrap(storage_value.value()))
    }

    /// Remove a value from storage and decode the result
    fn remove_decoded<V, Codec>(&mut self, key: &SlotKey, codec: &Codec) -> Option<V>
    where
        Codec: StateCodec,
        Codec::ValueCodec: StateItemCodec<V>,
    {
        let value = self.get_decoded(key, codec);
        self.delete(key);
        value
    }
}

/// A helper trait allowing a type to access any namespace by their *runtime* enum variant.
trait UniversalStateAccessor {
    fn get(&mut self, namespace: Namespace, key: &SlotKey) -> Option<SlotValue>;
    fn set(&mut self, namespace: Namespace, key: &SlotKey, value: SlotValue);
    fn delete(&mut self, namespace: Namespace, key: &SlotKey);
}

#[cfg(feature = "native")]
/// Allows a type to retrieve state values with a proof of their presence/absence.
pub trait ProvenStateAccessor<N: ProvableCompileTimeNamespace>: StateReaderAndWriter<N> {
    /// The underlying storage whose proof is returned
    type Proof;
    /// Fetch the value with the requested key and provide a proof of its presence/absence.
    fn get_with_proof(&mut self, key: SlotKey) -> StorageProof<Self::Proof>
    where
        Self: StateReaderAndWriter<N>,
        N: ProvableCompileTimeNamespace;
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
    user_cache: ProvableStorageCache<User>,
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

impl<S: Storage> StateReaderAndWriter<Accessory> for AccessoryDelta<S> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        if let Some(value) = self.writes.get(key) {
            return value.clone().map(Into::into);
        }
        self.storage.get_accessory(key, self.version)
    }

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

    /// Transforms this [`StateCheckpoint`] back into a [`WorkingSet`].
    pub fn to_revertable(self, gas_meter: GasMeter<S::Gas>) -> WorkingSet<S> {
        WorkingSet {
            delta: RevertableWriter::new(self.delta),
            events: Default::default(),
            gas_meter,
        }
    }

    /// Extracts ordered reads, writes, and witness from this [`StateCheckpoint`].
    ///
    /// You can then use these to call [`Storage::validate_and_commit`] or some
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

impl<N, S: Spec> StateReaderAndWriter<N> for StateCheckpoint<S>
where
    N: CompileTimeNamespace,
{
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        self.delta.get(N::NAMESPACE, key)
    }

    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        self.delta.set(N::NAMESPACE, key, value);
    }

    fn delete(&mut self, key: &SlotKey) {
        self.delta.delete(N::NAMESPACE, key);
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
    boxed_event: alloc::boxed::Box<dyn core::any::Any + core::marker::Send>,
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

/// This structure contains the read-write set and the events collected during the execution of a transaction.
/// There are two ways to convert it into a StateCheckpoint:
/// 1. By using the checkpoint() method, where all the changes are added to the underlying StateCheckpoint.
/// 2. By using the revert method, where the most recent changes are reverted and the previous `StateCheckpoint` is returned.
pub struct WorkingSet<S: Spec> {
    delta: RevertableWriter<Delta<S::Storage>>,
    events: Vec<TypedEvent>,
    gas_meter: GasMeter<S::Gas>,
}

impl<S: Spec> WorkingSet<S> {
    /// Creates a new [`WorkingSet`] instance backed by the given [`Storage`].
    ///
    /// The witness value is set to [`Default::default`]. Use
    /// [`WorkingSet::with_witness`] to set a custom witness value.
    pub fn new(inner: S::Storage) -> Self {
        StateCheckpoint::new(inner).to_revertable(Default::default())
    }

    /// Creates a new archival working set with the same underlying `Storage` but an empty Delta, without
    /// modifying the original [`WorkingSet`].
    pub fn get_archival_at(&self, version: u64) -> Self {
        let storage = self.delta.inner.inner.clone();
        Self {
            delta: RevertableWriter::new(Delta::new(storage.clone(), Some(version))),
            events: Default::default(),
            gas_meter: Default::default(),
        }
    }

    /// Returns a handler for the accessory state (non-JMT state).
    ///
    /// You can use this method when calling getters and setters on accessory
    /// state containers, like AccessoryStateMap.
    pub fn accessory_state(&mut self) -> AccessoryWorkingSet<S> {
        AccessoryWorkingSet { ws: self }
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
    pub fn with_witness(inner: S::Storage, witness: <S::Storage as Storage>::Witness) -> Self {
        StateCheckpoint::with_witness(inner, witness).to_revertable(Default::default())
    }

    /// Turns this [`WorkingSet`] into a [`StateCheckpoint`], in preparation for
    /// committing the changes to the underlying [`Storage`] via
    /// [`StateCheckpoint::freeze`].
    pub fn checkpoint(self) -> (StateCheckpoint<S>, GasMeter<S::Gas>, Vec<TypedEvent>) {
        (
            StateCheckpoint {
                delta: self.delta.commit(),
            },
            self.gas_meter,
            self.events,
        )
    }

    /// Reverts the most recent changes to this [`WorkingSet`], returning a pristine
    /// [`StateCheckpoint`] instance.
    pub fn revert(self) -> (StateCheckpoint<S>, GasMeter<S::Gas>) {
        (
            StateCheckpoint {
                delta: self.delta.revert(),
            },
            self.gas_meter,
        )
    }

    /// Adds a typed event to the working set.
    pub fn add_event<E: 'static + core::marker::Send>(&mut self, event_key: &str, event: E) {
        self.events.push(TypedEvent::new(event_key, event));
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
    pub const fn gas_remaining_funds(&self) -> u64 {
        self.gas_meter.remaining_funds()
    }

    /// Overrides the current gas funds available for transaction execution.
    pub fn set_gas_funds(&mut self, funds: u64) {
        self.gas_meter.set_gas_funds(funds);
    }

    /// Overrides the current gas price for transaction execution.
    pub fn set_gas_price(&mut self, gas_price: <S::Gas as Gas>::Price) {
        self.gas_meter.set_gas_price(gas_price);
    }

    /// Attempts to charge the provided gas unit from the gas meter, using the internal price to
    /// compute the scalar value.
    pub fn charge_gas(&mut self, gas: &S::Gas) -> anyhow::Result<()> {
        self.gas_meter.charge_gas(gas)
    }

    /// Returns the gas price.
    pub const fn gas_price(&self) -> &<S::Gas as Gas>::Price {
        self.gas_meter.gas_price()
    }

    /// Returns the total gas incurred.
    pub const fn gas_used(&self) -> &S::Gas {
        self.gas_meter.gas_used()
    }
}

#[cfg(feature = "native")]
impl<N: ProvableCompileTimeNamespace, S: Spec> ProvenStateAccessor<N> for WorkingSet<S>
where
    WorkingSet<S>: StateReaderAndWriter<N>,
{
    type Proof = <S::Storage as Storage>::Proof;

    fn get_with_proof(&mut self, key: SlotKey) -> StorageProof<Self::Proof> {
        self.delta
            .inner
            .inner
            .get_with_proof::<N>(key, self.delta.inner.version)
    }
}

impl<S: Spec> StateReaderAndWriter<User> for WorkingSet<S> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        self.delta.get(User::NAMESPACE, key)
    }

    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        self.delta.set(User::NAMESPACE, key, value);
    }

    fn delete(&mut self, key: &SlotKey) {
        self.delta.delete(User::NAMESPACE, key);
    }
}

/// A wrapper over [`WorkingSet`] that only allows access to the accessory
/// state (non-JMT state).
pub struct AccessoryWorkingSet<'a, S: Spec> {
    ws: &'a mut WorkingSet<S>,
}

impl<'a, S: Spec> StateReaderAndWriter<Accessory> for AccessoryWorkingSet<'a, S> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        if !cfg!(feature = "native") {
            None
        } else {
            self.ws.delta.get(Accessory::NAMESPACE, key)
        }
    }

    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        self.ws.delta.set(Accessory::NAMESPACE, key, value);
    }

    fn delete(&mut self, key: &SlotKey) {
        self.ws.delta.delete(Accessory::NAMESPACE, key);
    }
}

/// A wrapper over [`WorkingSet`] that only allows access to the accessory
/// state (non-JMT state).
pub struct AccessoryStateCheckpoint<'a, S: Spec> {
    checkpoint: &'a mut StateCheckpoint<S>,
}

impl<'a, S: Spec> StateReaderAndWriter<Accessory> for AccessoryStateCheckpoint<'a, S> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        if !cfg!(feature = "native") {
            None
        } else {
            self.checkpoint.delta.get(Accessory::NAMESPACE, key)
        }
    }

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

    use super::*;
    use crate::capabilities::Kernel;
    use crate::namespaces;

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

    impl<'a, S: Spec> StateReaderAndWriter<namespaces::Kernel>
        for VersionedStateReadWriter<'a, StateCheckpoint<S>>
    {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            self.ws.delta.get(namespaces::Kernel::NAMESPACE, key)
        }

        fn set(&mut self, key: &SlotKey, value: SlotValue) {
            self.ws.delta.set(namespaces::Kernel::NAMESPACE, key, value);
        }

        fn delete(&mut self, key: &SlotKey) {
            self.ws.delta.delete(namespaces::Kernel::NAMESPACE, key);
        }
    }

    impl<'a, S: Spec> StateReaderAndWriter<namespaces::Kernel>
        for VersionedStateReadWriter<'a, WorkingSet<S>>
    {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            self.ws.delta.get(namespaces::Kernel::NAMESPACE, key)
        }

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

    impl<'a, S: Spec> StateReaderAndWriter<User> for BootstrapWorkingSet<'a, S> {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            self.inner.delta.get(User::NAMESPACE, key)
        }

        fn set(&mut self, key: &SlotKey, value: SlotValue) {
            self.inner.delta.set(User::NAMESPACE, key, value);
        }

        fn delete(&mut self, key: &SlotKey) {
            self.inner.delta.delete(User::NAMESPACE, key);
        }
    }

    impl<'a, S: Spec> StateReaderAndWriter<namespaces::Kernel> for BootstrapWorkingSet<'a, S> {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            self.inner.delta.get(namespaces::Kernel::NAMESPACE, key)
        }

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

    impl<'a, S: Spec> StateReaderAndWriter<User> for KernelWorkingSet<'a, S> {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            self.inner.delta.get(User::NAMESPACE, key)
        }

        fn set(&mut self, key: &SlotKey, value: SlotValue) {
            self.inner.delta.set(User::NAMESPACE, key, value);
        }

        fn delete(&mut self, key: &SlotKey) {
            self.inner.delta.delete(User::NAMESPACE, key);
        }
    }

    impl<'a, S: Spec> StateReaderAndWriter<namespaces::Kernel> for KernelWorkingSet<'a, S> {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            self.inner.delta.get(namespaces::Kernel::NAMESPACE, key)
        }

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
