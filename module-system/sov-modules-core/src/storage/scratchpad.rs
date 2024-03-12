//! Runtime state machine definitions.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::Any;
use core::{fmt, mem};

pub use kernel_state::{KernelWorkingSet, VersionedStateReadWriter};
use sov_rollup_interface::maybestd::collections::HashMap;

use crate::common::{GasMeter, Prefix};
use crate::module::{Context, Spec};
use crate::namespaces::{Accessory, CompileTimeNamespace, User};
use crate::storage::{
    EncodeKeyLike, NativeStorage, OrderedReadsAndWrites, SlotKey, SlotValue, StateCodec,
    StateValueCodec, Storage, StorageInternalCache, StorageProof,
};
use crate::Gas;

/// A storage reader and writer
pub trait StateReaderAndWriter<N: CompileTimeNamespace> {
    /// Get a value from the storage.
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue>;

    /// Replaces a storage value.
    fn set(&mut self, key: &SlotKey, value: SlotValue);

    /// Deletes a storage value.
    fn delete(&mut self, key: &SlotKey);

    /// Replaces a storage value with the provided prefix, using the provided codec.
    fn set_value<Q, K, V, Codec>(
        &mut self,
        prefix: &Prefix,
        storage_key: &Q,
        value: &V,
        codec: &Codec,
    ) where
        Q: ?Sized,
        Codec: StateCodec,
        Codec::KeyCodec: EncodeKeyLike<Q, K>,
        Codec::ValueCodec: StateValueCodec<V>,
    {
        let storage_key = SlotKey::new(N::NAMESPACE, prefix, storage_key, codec.key_codec());
        let storage_value = SlotValue::new(value, codec.value_codec());
        self.set(&storage_key, storage_value);
    }

    /// Replaces a storage value with a singleton prefix. For more information, check
    /// [SlotKey::singleton].
    fn set_singleton<V, Codec>(&mut self, prefix: &Prefix, value: &V, codec: &Codec)
    where
        Codec: StateCodec,
        Codec::ValueCodec: StateValueCodec<V>,
    {
        let storage_key = SlotKey::singleton(N::NAMESPACE, prefix);
        let storage_value = SlotValue::new(value, codec.value_codec());
        self.set(&storage_key, storage_value);
    }

    /// Get a decoded value from the storage.
    fn get_decoded<V, Codec>(&mut self, storage_key: &SlotKey, codec: &Codec) -> Option<V>
    where
        Codec: StateCodec,
        Codec::ValueCodec: StateValueCodec<V>,
    {
        let storage_value = self.get(storage_key)?;

        Some(
            codec
                .value_codec()
                .decode_value_unwrap(storage_value.value()),
        )
    }

    /// Get a value from the storage.
    fn get_value<Q, K, V, Codec>(
        &mut self,
        prefix: &Prefix,
        storage_key: &Q,
        codec: &Codec,
    ) -> Option<V>
    where
        Q: ?Sized,
        Codec: StateCodec,
        Codec::KeyCodec: EncodeKeyLike<Q, K>,
        Codec::ValueCodec: StateValueCodec<V>,
    {
        let storage_key = SlotKey::new(N::NAMESPACE, prefix, storage_key, codec.key_codec());
        self.get_decoded(&storage_key, codec)
    }

    /// Get a singleton value from the storage. For more information, check [SlotKey::singleton].
    fn get_singleton<V, Codec>(&mut self, prefix: &Prefix, codec: &Codec) -> Option<V>
    where
        Codec: StateCodec,
        Codec::ValueCodec: StateValueCodec<V>,
    {
        let storage_key = SlotKey::singleton(N::NAMESPACE, prefix);
        self.get_decoded(&storage_key, codec)
    }

    /// Removes a value from the storage.
    fn remove_value<Q, K, V, Codec>(
        &mut self,
        prefix: &Prefix,
        storage_key: &Q,
        codec: &Codec,
    ) -> Option<V>
    where
        Q: ?Sized,
        Codec: StateCodec,
        Codec::KeyCodec: EncodeKeyLike<Q, K>,
        Codec::ValueCodec: StateValueCodec<V>,
    {
        let storage_key = SlotKey::new(N::NAMESPACE, prefix, storage_key, codec.key_codec());
        let storage_value = self.get_decoded(&storage_key, codec)?;
        self.delete(&storage_key);
        Some(storage_value)
    }

    /// Removes a singleton from the storage. For more information, check [SlotKey::singleton].
    fn remove_singleton<V, Codec>(&mut self, prefix: &Prefix, codec: &Codec) -> Option<V>
    where
        Codec: StateCodec,
        Codec::ValueCodec: StateValueCodec<V>,
    {
        let storage_key = SlotKey::singleton(N::NAMESPACE, prefix);
        let storage_value = self.get_decoded(&storage_key, codec)?;
        self.delete(&storage_key);
        Some(storage_value)
    }

    /// Deletes a value from the storage.
    fn delete_value<Q, K, Codec>(&mut self, prefix: &Prefix, storage_key: &Q, codec: &Codec)
    where
        Q: ?Sized,
        Codec: StateCodec,
        Codec::KeyCodec: EncodeKeyLike<Q, K>,
    {
        let storage_key = SlotKey::new(N::NAMESPACE, prefix, storage_key, codec.key_codec());
        self.delete(&storage_key);
    }

    /// Deletes a singleton from the storage. For more information, check [SlotKey::singleton].
    fn delete_singleton(&mut self, prefix: &Prefix) {
        let storage_key = SlotKey::singleton(N::NAMESPACE, prefix);
        self.delete(&storage_key);
    }
}

/// A working set accumulates reads and writes on top of the underlying DB,
/// automating witness creation.
pub struct Delta<S: Storage> {
    inner: S,
    witness: S::Witness,
    cache: StorageInternalCache,
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
            cache: Default::default(),
            version,
        }
    }

    fn freeze(&mut self) -> (OrderedReadsAndWrites, S::Witness) {
        let cache = mem::take(&mut self.cache);
        let witness = mem::take(&mut self.witness);

        (cache.into(), witness)
    }
}

impl<S: Storage> fmt::Debug for Delta<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Delta").finish()
    }
}

impl<S: Storage> StateReaderAndWriter<User> for Delta<S> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        self.cache
            .get_or_fetch(key, &self.inner, &self.witness, self.version)
    }

    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        self.cache.set(key, value)
    }

    fn delete(&mut self, key: &SlotKey) {
        self.cache.delete(key)
    }
}

// type RevertableWrites = HashMap<SlotKey, Option<SlotValue>>;

#[derive(Default)]
struct RevertableWrites {
    pub cache: HashMap<SlotKey, Option<SlotValue>>,
    pub version: Option<u64>,
}

struct AccessoryDelta<S: Storage> {
    // This inner storage is never accessed inside the zkVM because reads are
    // not allowed, so it can result as dead code.
    #[allow(dead_code)]
    storage: S,
    writes: RevertableWrites,
}

impl<S: Storage> AccessoryDelta<S> {
    fn new(storage: S, version: Option<u64>) -> Self {
        let writes = match version {
            None => Default::default(),
            Some(v) => RevertableWrites {
                cache: Default::default(),
                version: Some(v),
            },
        };
        Self { storage, writes }
    }

    fn freeze(&mut self) -> OrderedReadsAndWrites {
        let mut reads_and_writes = OrderedReadsAndWrites::default();
        let writes = mem::take(&mut self.writes);

        for write in writes.cache {
            reads_and_writes.ordered_writes.push((write.0, write.1));
        }

        reads_and_writes
    }
}

impl<S: Storage> StateReaderAndWriter<Accessory> for AccessoryDelta<S> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        if let Some(value) = self.writes.cache.get(key) {
            return value.clone().map(Into::into);
        }
        self.storage.get_accessory(key, self.writes.version)
    }

    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        self.writes.cache.insert(key.clone(), Some(value));
    }

    fn delete(&mut self, key: &SlotKey) {
        self.writes.cache.insert(key.clone(), None);
    }
}

/// This structure is responsible for storing the `read-write` set.
///
/// A [`StateCheckpoint`] can be obtained from a [`WorkingSet`] in two ways:
///  1. With [`WorkingSet::checkpoint`].
///  2. With [`WorkingSet::revert`].
pub struct StateCheckpoint<S: Spec> {
    delta: Delta<S::Storage>,
    accessory_delta: AccessoryDelta<S::Storage>,
}

impl<S: Spec> StateCheckpoint<S> {
    /// Creates a new [`StateCheckpoint`] instance without any changes, backed
    /// by the given [`Storage`].
    pub fn new(inner: S::Storage) -> Self {
        Self {
            delta: Delta::new(inner.clone(), None),
            accessory_delta: AccessoryDelta::new(inner, None),
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
            accessory_delta: AccessoryDelta::new(inner, None),
        }
    }

    /// Transforms this [`StateCheckpoint`] back into a [`WorkingSet`].
    pub fn to_revertable(self, gas_meter: GasMeter<S::Gas>) -> WorkingSet<S> {
        WorkingSet {
            delta: RevertableWriter::new(self.delta),
            accessory_delta: RevertableWriter::new(self.accessory_delta),
            events: Default::default(),
            gas_meter,
        }
    }

    /// Extracts ordered reads, writes, and witness from this [`StateCheckpoint`].
    ///
    /// You can then use these to call [`Storage::validate_and_commit`] or some
    /// of the other related [`Storage`] methods. Note that this data is moved
    /// **out** of the [`StateCheckpoint`] i.e. it can't be extracted twice.
    pub fn freeze(&mut self) -> (OrderedReadsAndWrites, <S::Storage as Storage>::Witness) {
        self.delta.freeze()
    }

    /// Extracts ordered reads and writes of accessory state from this
    /// [`StateCheckpoint`].
    ///
    /// You can then use these to call
    /// [`Storage::validate_and_commit_with_accessory_update`], together with
    /// the data extracted with [`StateCheckpoint::freeze`].
    pub fn freeze_non_provable(&mut self) -> OrderedReadsAndWrites {
        self.accessory_delta.freeze()
    }
}

impl<S: Spec> StateReaderAndWriter<User> for StateCheckpoint<S> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        self.delta.get(key)
    }

    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        self.delta.set(key, value)
    }

    fn delete(&mut self, key: &SlotKey) {
        self.delta.delete(key)
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
pub struct TypedEvent<S: Spec> {
    event_key: Vec<u8>,
    module_address: S::Address,
    type_id: core::any::TypeId,
    boxed_event: alloc::boxed::Box<dyn core::any::Any + core::marker::Send>,
}

impl<S: Spec> TypedEvent<S> {
    /// Created a Typed Event
    pub fn new<E: 'static + core::marker::Send>(
        event_key: &str,
        module_address: S::Address,
        event: E,
    ) -> Self {
        TypedEvent {
            event_key: event_key.as_bytes().to_vec(),
            module_address,
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

    /// Function to peek at the module address
    pub fn module_address(&self) -> &S::Address {
        &self.module_address
    }
}

/// This structure contains the read-write set and the events collected during the execution of a transaction.
/// There are two ways to convert it into a StateCheckpoint:
/// 1. By using the checkpoint() method, where all the changes are added to the underlying StateCheckpoint.
/// 2. By using the revert method, where the most recent changes are reverted and the previous `StateCheckpoint` is returned.
pub struct WorkingSet<S: Spec> {
    delta: RevertableWriter<Delta<S::Storage>>,
    accessory_delta: RevertableWriter<AccessoryDelta<S::Storage>>,
    events: Vec<TypedEvent<S>>,
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
            accessory_delta: RevertableWriter::new(AccessoryDelta::new(storage, Some(version))),
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
    pub fn checkpoint(self) -> (StateCheckpoint<S>, GasMeter<S::Gas>, Vec<TypedEvent<S>>) {
        (
            StateCheckpoint {
                delta: self.delta.commit(),
                accessory_delta: self.accessory_delta.commit(),
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
                accessory_delta: self.accessory_delta.revert(),
            },
            self.gas_meter,
        )
    }

    /// Adds a typed event to the working set.
    pub fn add_event<E: 'static + core::marker::Send>(
        &mut self,
        event_key: &str,
        module_address: &S::Address,
        event: E,
    ) {
        self.events
            .push(TypedEvent::new(event_key, module_address.clone(), event));
    }

    /// Extracts all typed events from this working set.
    pub fn take_events(&mut self) -> Vec<TypedEvent<S>> {
        core::mem::take(&mut self.events)
    }

    /// Extracts a typed event at index `index`
    pub fn take_event(&mut self, index: usize) -> Option<TypedEvent<S>> {
        if index < self.events.len() {
            Some(self.events.remove(index))
        } else {
            None
        }
    }

    /// Returns an immutable map of all typed events that have been previously
    /// written to this working set.
    pub fn events(&self) -> &[TypedEvent<S>] {
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

    /// Fetches given value and provides a proof of it presence/absence.
    pub fn get_with_proof(&mut self, key: SlotKey) -> StorageProof<<S::Storage as Storage>::Proof>
    where
        S::Storage: NativeStorage,
    {
        // First inner is `RevertableWriter` and second inner is actually a `Storage` instance
        self.delta.inner.inner.get_with_proof(key)
    }
}

impl<S: Spec> StateReaderAndWriter<User> for WorkingSet<S> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        self.delta.get(key)
    }

    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        self.delta.set(key, value)
    }

    fn delete(&mut self, key: &SlotKey) {
        self.delta.delete(key);
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
            self.ws.accessory_delta.get(key)
        }
    }

    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        self.ws.accessory_delta.set(key, value);
    }

    fn delete(&mut self, key: &SlotKey) {
        self.ws.accessory_delta.delete(key);
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
            self.checkpoint.accessory_delta.get(key)
        }
    }

    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        self.checkpoint.accessory_delta.set(key, value)
    }

    fn delete(&mut self, key: &SlotKey) {
        self.checkpoint.accessory_delta.delete(key)
    }
}

/// Provides specialized working set wrappers for dealing with protected state.
pub mod kernel_state {
    use sov_rollup_interface::da::DaSpec;

    use super::*;
    use crate::capabilities::Kernel;
    use crate::namespaces;

    /// A trait indicating that this working set is version aware
    pub trait VersionReader: StateReaderAndWriter<crate::namespaces::Kernel> {
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

    impl<'a, S: Spec> StateReaderAndWriter<crate::namespaces::Kernel>
        for VersionedStateReadWriter<'a, StateCheckpoint<S>>
    {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            self.ws.delta.get(key)
        }

        fn set(&mut self, key: &SlotKey, value: SlotValue) {
            self.ws.delta.set(key, value)
        }

        fn delete(&mut self, key: &SlotKey) {
            self.ws.delta.delete(key)
        }
    }

    impl<'a, S: Spec> StateReaderAndWriter<crate::namespaces::Kernel>
        for VersionedStateReadWriter<'a, WorkingSet<S>>
    {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            self.ws.delta.get(key)
        }

        fn set(&mut self, key: &SlotKey, value: SlotValue) {
            self.ws.delta.set(key, value)
        }

        fn delete(&mut self, key: &SlotKey) {
            self.ws.delta.delete(key)
        }
    }

    /// A special wrapper over [`WorkingSet`] that allows access to kernel values to bootstrap the kernel working set
    pub struct BootstrapWorkingSet<'a, S: Spec> {
        /// The inner working set
        pub(crate) inner: &'a mut StateCheckpoint<S>,
    }

    impl<'a, S: Spec> StateReaderAndWriter<User> for BootstrapWorkingSet<'a, S> {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            self.inner.delta.get(key)
        }

        fn set(&mut self, key: &SlotKey, value: SlotValue) {
            self.inner.delta.set(key, value)
        }

        fn delete(&mut self, key: &SlotKey) {
            self.inner.delta.delete(key)
        }
    }

    impl<'a, S: Spec> StateReaderAndWriter<namespaces::Kernel> for BootstrapWorkingSet<'a, S> {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            self.inner.delta.get(key)
        }

        fn set(&mut self, key: &SlotKey, value: SlotValue) {
            self.inner.delta.set(key, value)
        }

        fn delete(&mut self, key: &SlotKey) {
            self.inner.delta.delete(key)
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
            self.inner.delta.get(key)
        }

        fn set(&mut self, key: &SlotKey, value: SlotValue) {
            self.inner.delta.set(key, value)
        }

        fn delete(&mut self, key: &SlotKey) {
            self.inner.delta.delete(key)
        }
    }

    impl<'a, S: Spec> StateReaderAndWriter<crate::namespaces::Kernel> for KernelWorkingSet<'a, S> {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            self.inner.delta.get(key)
        }

        fn set(&mut self, key: &SlotKey, value: SlotValue) {
            self.inner.delta.set(key, value)
        }

        fn delete(&mut self, key: &SlotKey) {
            self.inner.delta.delete(key)
        }
    }
}

struct RevertableWriter<T> {
    inner: T,
    writes: HashMap<SlotKey, Option<SlotValue>>,
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
    fn commit<N: CompileTimeNamespace>(mut self) -> T
    where
        T: StateReaderAndWriter<N>,
    {
        for (k, v) in self.writes.into_iter() {
            Self::commit_entry(&mut self.inner, k, v);
        }

        self.inner
    }

    fn revert(self) -> T {
        self.inner
    }

    fn commit_entry<N: CompileTimeNamespace>(inner: &mut T, key: SlotKey, value: Option<SlotValue>)
    where
        T: StateReaderAndWriter<N>,
    {
        match value {
            Some(value) => inner.set(&key, value),
            None => inner.delete(&key),
        }
    }
}

impl<T, N: CompileTimeNamespace> StateReaderAndWriter<N> for RevertableWriter<T>
where
    T: StateReaderAndWriter<N>,
{
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        if let Some(value) = self.writes.get(key) {
            value.as_ref().cloned().map(Into::into)
        } else {
            self.inner.get(key)
        }
    }

    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        self.writes.insert(key.clone(), Some(value));
    }

    fn delete(&mut self, key: &SlotKey) {
        self.writes.insert(key.clone(), None);
    }
}
