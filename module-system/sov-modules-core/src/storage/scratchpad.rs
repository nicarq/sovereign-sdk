//! Runtime state machine definitions.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::Any;
use core::{fmt, mem};

pub use kernel_state::{KernelWorkingSet, VersionedStateReadWriter};
use sov_rollup_interface::maybestd::collections::HashMap;

use crate::archival_state::{ArchivalAccessoryWorkingSet, ArchivalJmtWorkingSet};
use crate::common::{GasMeter, Prefix};
use crate::module::{Context, Spec};
use crate::storage::{
    EncodeKeyLike, NativeStorage, OrderedReadsAndWrites, SlotKey, SlotValue, StateCodec,
    StateValueCodec, Storage, StorageInternalCache, StorageProof,
};
use crate::{Gas, Version};

/// A storage reader and writer
pub trait StateReaderAndWriter {
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
        let storage_key = SlotKey::new(prefix, storage_key, codec.key_codec());
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
        let storage_key = SlotKey::singleton(prefix);
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
        let storage_key = SlotKey::new(prefix, storage_key, codec.key_codec());
        self.get_decoded(&storage_key, codec)
    }

    /// Get a singleton value from the storage. For more information, check [SlotKey::singleton].
    fn get_singleton<V, Codec>(&mut self, prefix: &Prefix, codec: &Codec) -> Option<V>
    where
        Codec: StateCodec,
        Codec::ValueCodec: StateValueCodec<V>,
    {
        let storage_key = SlotKey::singleton(prefix);
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
        let storage_key = SlotKey::new(prefix, storage_key, codec.key_codec());
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
        let storage_key = SlotKey::singleton(prefix);
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
        let storage_key = SlotKey::new(prefix, storage_key, codec.key_codec());
        self.delete(&storage_key);
    }

    /// Deletes a singleton from the storage. For more information, check [SlotKey::singleton].
    fn delete_singleton(&mut self, prefix: &Prefix) {
        let storage_key = SlotKey::singleton(prefix);
        self.delete(&storage_key);
    }
}

/// A working set accumulates reads and writes on top of the underlying DB,
/// automating witness creation.
pub struct Delta<S: Storage> {
    inner: S,
    witness: S::Witness,
    cache: StorageInternalCache,
}

impl<S: Storage> Delta<S> {
    fn new(inner: S, version: Option<u64>) -> Self {
        Self::with_witness(inner, Default::default(), version)
    }

    fn with_witness(inner: S, witness: S::Witness, version: Option<u64>) -> Self {
        Self {
            inner,
            witness,
            cache: match version {
                None => Default::default(),
                Some(v) => StorageInternalCache::new_with_version(v),
            },
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

impl<S: Storage> StateReaderAndWriter for Delta<S> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        self.cache.get_or_fetch(key, &self.inner, &self.witness)
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

impl<S: Storage> StateReaderAndWriter for AccessoryDelta<S> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        let cache_key = key.to_cache_key_version(self.writes.version);
        if let Some(value) = self.writes.cache.get(&cache_key) {
            return value.clone().map(Into::into);
        }
        self.storage.get_accessory(key, self.writes.version)
    }

    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        self.writes
            .cache
            .insert(key.to_cache_key_version(self.writes.version), Some(value));
    }

    fn delete(&mut self, key: &SlotKey) {
        self.writes
            .cache
            .insert(key.to_cache_key_version(self.writes.version), None);
    }
}

/// This structure is responsible for storing the `read-write` set.
///
/// A [`StateCheckpoint`] can be obtained from a [`WorkingSet`] in two ways:
///  1. With [`WorkingSet::checkpoint`].
///  2. With [`WorkingSet::revert`].
pub struct StateCheckpoint<C: Context> {
    delta: Delta<C::Storage>,
    accessory_delta: AccessoryDelta<C::Storage>,
}

impl<C: Context> StateCheckpoint<C> {
    /// Creates a new [`StateCheckpoint`] instance without any changes, backed
    /// by the given [`Storage`].
    pub fn new(inner: <C as Spec>::Storage) -> Self {
        Self {
            delta: Delta::new(inner.clone(), None),
            accessory_delta: AccessoryDelta::new(inner, None),
        }
    }

    /// Returns a handler for the accessory state (non-JMT state).
    ///
    /// You can use this method when calling getters and setters on accessory
    /// state containers, like AccessoryStateMap.
    pub fn accessory_state(&mut self) -> AccessoryStateCheckpoint<C> {
        AccessoryStateCheckpoint { checkpoint: self }
    }

    /// Returns a handler for the kernel state (priveleged jmt state)
    ///
    /// You can use this method when calling getters and setters on accessory
    /// state containers, like KernelStateMap.
    pub fn versioned_state(&mut self, context: &C) -> VersionedStateReadWriter<Self> {
        VersionedStateReadWriter {
            ws: self,
            slot_num: context.visible_slot_number(),
        }
    }

    /// Creates a new [`StateCheckpoint`] instance without any changes, backed
    /// by the given [`Storage`] and witness.
    pub fn with_witness(
        inner: <C as Spec>::Storage,
        witness: <<C as Spec>::Storage as Storage>::Witness,
    ) -> Self {
        Self {
            delta: Delta::with_witness(inner.clone(), witness, None),
            accessory_delta: AccessoryDelta::new(inner, None),
        }
    }

    /// Transforms this [`StateCheckpoint`] back into a [`WorkingSet`].
    pub fn to_revertable(self, gas_meter: GasMeter<C::Gas>) -> WorkingSet<C> {
        WorkingSet {
            delta: RevertableWriter::new(self.delta, None),
            accessory_delta: RevertableWriter::new(self.accessory_delta, None),
            events: Default::default(),
            gas_meter,
            archival_working_set: None,
            archival_accessory_working_set: None,
        }
    }

    /// Extracts ordered reads, writes, and witness from this [`StateCheckpoint`].
    ///
    /// You can then use these to call [`Storage::validate_and_commit`] or some
    /// of the other related [`Storage`] methods. Note that this data is moved
    /// **out** of the [`StateCheckpoint`] i.e. it can't be extracted twice.
    pub fn freeze(
        &mut self,
    ) -> (
        OrderedReadsAndWrites,
        <<C as Spec>::Storage as Storage>::Witness,
    ) {
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

impl<C: Context> StateReaderAndWriter for StateCheckpoint<C> {
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
pub struct TypedEvent<C: Context> {
    event_key: Vec<u8>,
    module_address: <C as Spec>::Address,
    type_id: core::any::TypeId,
    boxed_event: alloc::boxed::Box<dyn core::any::Any + core::marker::Send>,
}

impl<C: Context> TypedEvent<C> {
    /// Created a Typed Event
    pub fn new<E: 'static + core::marker::Send>(
        event_key: &str,
        module_address: <C as Spec>::Address,
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
    pub fn module_address(&self) -> &<C as Spec>::Address {
        &self.module_address
    }
}

/// This structure contains the read-write set and the events collected during the execution of a transaction.
/// There are two ways to convert it into a StateCheckpoint:
/// 1. By using the checkpoint() method, where all the changes are added to the underlying StateCheckpoint.
/// 2. By using the revert method, where the most recent changes are reverted and the previous `StateCheckpoint` is returned.
pub struct WorkingSet<C: Context> {
    delta: RevertableWriter<Delta<C::Storage>>,
    accessory_delta: RevertableWriter<AccessoryDelta<C::Storage>>,
    events: Vec<TypedEvent<C>>,
    gas_meter: GasMeter<C::Gas>,
    archival_working_set: Option<ArchivalJmtWorkingSet<C>>,
    archival_accessory_working_set: Option<ArchivalAccessoryWorkingSet<C>>,
}

impl<C: Context> WorkingSet<C> {
    /// Creates a new [`WorkingSet`] instance backed by the given [`Storage`].
    ///
    /// The witness value is set to [`Default::default`]. Use
    /// [`WorkingSet::with_witness`] to set a custom witness value.
    pub fn new(inner: <C as Spec>::Storage) -> Self {
        StateCheckpoint::new(inner).to_revertable(Default::default())
    }

    /// Returns a handler for the accessory state (non-JMT state).
    ///
    /// You can use this method when calling getters and setters on accessory
    /// state containers, like AccessoryStateMap.
    pub fn accessory_state(&mut self) -> AccessoryWorkingSet<C> {
        AccessoryWorkingSet { ws: self }
    }

    /// Returns a handler for the archival state (JMT state).
    fn archival_state(&mut self, version: Version) -> ArchivalJmtWorkingSet<C> {
        ArchivalJmtWorkingSet::new(&self.delta.inner.inner, version)
    }

    /// Returns a handler for the archival accessory state (non-JMT state).
    fn archival_accessory_state(&mut self, version: Version) -> ArchivalAccessoryWorkingSet<C> {
        ArchivalAccessoryWorkingSet::new(&self.accessory_delta.inner.storage, version)
    }

    /// Sets archival version for a working set
    pub fn set_archival_version(&mut self, version: Version) {
        self.archival_working_set = Some(self.archival_state(version));
        self.archival_accessory_working_set = Some(self.archival_accessory_state(version));
    }

    /// Unset archival version
    pub fn unset_archival_version(&mut self) {
        self.archival_working_set = None;
        self.archival_accessory_working_set = None;
    }

    /// Returns a handler for the kernel state (priveleged jmt state)
    ///
    /// You can use this method when calling getters and setters on accessory
    /// state containers, like KernelStateMap.
    pub fn versioned_state(&mut self, context: &C) -> VersionedStateReadWriter<Self> {
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
    pub fn with_witness(
        inner: <C as Spec>::Storage,
        witness: <<C as Spec>::Storage as Storage>::Witness,
    ) -> Self {
        StateCheckpoint::with_witness(inner, witness).to_revertable(Default::default())
    }

    /// Turns this [`WorkingSet`] into a [`StateCheckpoint`], in preparation for
    /// committing the changes to the underlying [`Storage`] via
    /// [`StateCheckpoint::freeze`].
    pub fn checkpoint(self) -> (StateCheckpoint<C>, GasMeter<C::Gas>, Vec<TypedEvent<C>>) {
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
    pub fn revert(self) -> (StateCheckpoint<C>, GasMeter<C::Gas>) {
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
        module_address: &<C as Spec>::Address,
        event: E,
    ) {
        self.events
            .push(TypedEvent::new(event_key, module_address.clone(), event));
    }

    /// Extracts all typed events from this working set.
    pub fn take_events(&mut self) -> Vec<TypedEvent<C>> {
        core::mem::take(&mut self.events)
    }

    /// Extracts a typed event at index `index`
    pub fn take_event(&mut self, index: usize) -> Option<TypedEvent<C>> {
        if index < self.events.len() {
            Some(self.events.remove(index))
        } else {
            None
        }
    }

    /// Returns an immutable map of all typed events that have been previously
    /// written to this working set.
    pub fn events(&self) -> &[TypedEvent<C>] {
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
    pub fn set_gas_price(&mut self, gas_price: <C::Gas as Gas>::Price) {
        self.gas_meter.set_gas_price(gas_price);
    }

    /// Attempts to charge the provided gas unit from the gas meter, using the internal price to
    /// compute the scalar value.
    pub fn charge_gas(&mut self, gas: &C::Gas) -> anyhow::Result<()> {
        self.gas_meter.charge_gas(gas)
    }

    /// Returns the gas price.
    pub const fn gas_price(&self) -> &<C::Gas as Gas>::Price {
        self.gas_meter.gas_price()
    }

    /// Returns the total gas incurred.
    pub const fn gas_used(&self) -> &C::Gas {
        self.gas_meter.gas_used()
    }

    /// Fetches given value and provides a proof of it presence/absence.
    pub fn get_with_proof(&mut self, key: SlotKey) -> StorageProof<<C::Storage as Storage>::Proof>
    where
        C::Storage: NativeStorage,
    {
        // First inner is `RevertableWriter` and second inner is actually a `Storage` instance
        self.delta.inner.inner.get_with_proof(key)
    }
}

impl<C: Context> StateReaderAndWriter for WorkingSet<C> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        match &mut self.archival_working_set {
            None => self.delta.get(key),
            Some(ref mut archival_working_set) => archival_working_set.get(key),
        }
    }

    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        match &mut self.archival_working_set {
            None => self.delta.set(key, value),
            Some(ref mut archival_working_set) => archival_working_set.set(key, value),
        }
    }

    fn delete(&mut self, key: &SlotKey) {
        match &mut self.archival_working_set {
            None => self.delta.delete(key),
            Some(ref mut archival_working_set) => archival_working_set.delete(key),
        }
    }
}

/// A wrapper over [`WorkingSet`] that only allows access to the accessory
/// state (non-JMT state).
pub struct AccessoryWorkingSet<'a, C: Context> {
    ws: &'a mut WorkingSet<C>,
}

impl<'a, C: Context> StateReaderAndWriter for AccessoryWorkingSet<'a, C> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        if !cfg!(feature = "native") {
            None
        } else {
            match &mut self.ws.archival_accessory_working_set {
                None => self.ws.accessory_delta.get(key),
                Some(ref mut archival_working_set) => archival_working_set.get(key),
            }
        }
    }

    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        match &mut self.ws.archival_accessory_working_set {
            None => self.ws.accessory_delta.set(key, value),
            Some(ref mut archival_working_set) => archival_working_set.set(key, value),
        }
    }

    fn delete(&mut self, key: &SlotKey) {
        match &mut self.ws.archival_accessory_working_set {
            None => self.ws.accessory_delta.delete(key),
            Some(ref mut archival_working_set) => archival_working_set.delete(key),
        }
    }
}

/// A wrapper over [`WorkingSet`] that only allows access to the accessory
/// state (non-JMT state).
pub struct AccessoryStateCheckpoint<'a, C: Context> {
    checkpoint: &'a mut StateCheckpoint<C>,
}

impl<'a, C: Context> StateReaderAndWriter for AccessoryStateCheckpoint<'a, C> {
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

/// Module for archival state
pub mod archival_state {
    use super::*;

    /// Archival JMT
    pub struct ArchivalJmtWorkingSet<C: Context> {
        delta: RevertableWriter<Delta<C::Storage>>,
    }

    impl<C: Context> ArchivalJmtWorkingSet<C> {
        /// create a new instance of ArchivalJmtWorkingSet
        pub fn new(inner: &<C as Spec>::Storage, version: Version) -> Self {
            Self {
                delta: RevertableWriter::new(
                    Delta::new(inner.clone(), Some(version)),
                    Some(version),
                ),
            }
        }
    }

    /// Archival Accessory
    pub struct ArchivalAccessoryWorkingSet<C: Context> {
        delta: RevertableWriter<AccessoryDelta<C::Storage>>,
    }

    impl<C: Context> ArchivalAccessoryWorkingSet<C> {
        /// create a new instance of ArchivalAccessoryWorkingSet
        pub fn new(inner: &<C as Spec>::Storage, version: Version) -> Self {
            Self {
                delta: RevertableWriter::new(
                    AccessoryDelta::new(inner.clone(), Some(version)),
                    Some(version),
                ),
            }
        }
    }

    impl<C: Context> StateReaderAndWriter for ArchivalJmtWorkingSet<C> {
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

    impl<C: Context> StateReaderAndWriter for ArchivalAccessoryWorkingSet<C> {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            if !cfg!(feature = "native") {
                None
            } else {
                self.delta.get(key)
            }
        }

        fn set(&mut self, key: &SlotKey, value: SlotValue) {
            self.delta.set(key, value)
        }

        fn delete(&mut self, key: &SlotKey) {
            self.delta.delete(key)
        }
    }
}

/// Provides specialized working set wrappers for dealing with protected state.
pub mod kernel_state {
    use sov_rollup_interface::da::DaSpec;

    use super::*;
    use crate::capabilities::Kernel;

    /// A trait indicating that this working set is version aware
    pub trait VersionReader: StateReaderAndWriter {
        /// Returns the current version of the working set
        fn current_version(&self) -> u64;
    }

    impl<'a, S: StateReaderAndWriter> VersionReader for VersionedStateReadWriter<'a, S> {
        fn current_version(&self) -> u64 {
            self.slot_num
        }
    }

    /// A wrapper over [`WorkingSet`] that allows access to kernel values
    pub struct VersionedStateReadWriter<'a, S> {
        pub(super) ws: &'a mut S,
        pub(super) slot_num: u64,
    }

    impl<'a, C: Context> VersionedStateReadWriter<'a, StateCheckpoint<C>> {
        /// Instantiates a [`VersionedStateReadWriter`] from a kernel working set.
        /// Sets the `slot_num` to the virtual slot number of the kernel.
        pub fn from_kernel_ws_virtual(
            kernel_ws: KernelWorkingSet<'a, C>,
        ) -> VersionedStateReadWriter<'a, StateCheckpoint<C>> {
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

    impl<'a, S: StateReaderAndWriter> StateReaderAndWriter for VersionedStateReadWriter<'a, S> {
        fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
            self.ws.get(key)
        }

        fn set(&mut self, key: &SlotKey, value: SlotValue) {
            self.ws.set(key, value)
        }

        fn delete(&mut self, key: &SlotKey) {
            self.ws.delete(key)
        }
    }

    /// A special wrapper over [`WorkingSet`] that allows access to kernel values to bootstrap the kernel working set
    pub struct BootstrapWorkingSet<'a, C: Context> {
        /// The inner working set
        pub(crate) inner: &'a mut StateCheckpoint<C>,
    }

    impl<'a, C: Context> StateReaderAndWriter for BootstrapWorkingSet<'a, C> {
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
    pub struct KernelWorkingSet<'a, C: Context> {
        /// The inner working set
        pub inner: &'a mut StateCheckpoint<C>,
        /// The actual current slot number
        pub(super) true_slot_num: u64,
        /// The slot number visible to user-space modules
        pub(super) virtual_slot_num: u64,
    }

    impl<'a, C: Context> VersionReader for KernelWorkingSet<'a, C> {
        fn current_version(&self) -> u64 {
            self.true_slot_num
        }
    }

    impl<'a, C: Context> KernelWorkingSet<'a, C> {
        /// This private method instantiates a bootstrap working set to initialize a kernel
        fn get_bootstrap(inner: &'a mut StateCheckpoint<C>) -> BootstrapWorkingSet<'a, C> {
            BootstrapWorkingSet { inner }
        }

        /// Build a new kernel working set from the associated kernel
        pub fn from_kernel<K: Kernel<C, Da>, Da: DaSpec>(
            kernel: &K,
            ws: &'a mut StateCheckpoint<C>,
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
        pub fn uninitialized(ws: &'a mut StateCheckpoint<C>) -> Self {
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

    impl<'a, C: Context> StateReaderAndWriter for KernelWorkingSet<'a, C> {
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
    version: Option<u64>,
}

impl<T: fmt::Debug> fmt::Debug for RevertableWriter<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RevertableWriter")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<T> RevertableWriter<T>
where
    T: StateReaderAndWriter,
{
    fn new(inner: T, version: Option<u64>) -> Self {
        Self {
            inner,
            writes: Default::default(),
            version,
        }
    }

    /// Commit all items from `RevertableWriter` returning the inner storage.
    fn commit(mut self) -> T {
        for (k, v) in self.writes.into_iter() {
            Self::commit_entry(&mut self.inner, k, v);
        }

        self.inner
    }

    fn revert(self) -> T {
        self.inner
    }

    fn commit_entry(inner: &mut T, key: SlotKey, value: Option<SlotValue>) {
        if let Some(value) = value {
            inner.set(&key, value);
        } else {
            inner.delete(&key);
        }
    }
}

impl<T: StateReaderAndWriter> StateReaderAndWriter for RevertableWriter<T> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        if let Some(value) = self.writes.get(&key.to_cache_key_version(self.version)) {
            value.as_ref().cloned().map(Into::into)
        } else {
            self.inner.get(key)
        }
    }

    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        self.writes
            .insert(key.to_cache_key_version(self.version), Some(value));
    }

    fn delete(&mut self, key: &SlotKey) {
        self.writes
            .insert(key.to_cache_key_version(self.version), None);
    }
}
