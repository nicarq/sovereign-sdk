//! Defines traits for storage access

#[cfg(feature = "native")]
use crate::namespaces::ProvableCompileTimeNamespace;
use crate::namespaces::{Accessory, CompileTimeNamespace, User};
use crate::storage::codec::StateItemDecoder;
#[cfg(feature = "native")]
use crate::StorageProof;
use crate::{Namespace, SlotKey, SlotValue, Spec, StateCodec, StateItemCodec};

/// The state accessor used during transaction execution. It provides unrestricted
/// access to [`User`]-space state, as well as limited visibility into the `Kernel` state.
pub trait TxState<S: Spec>:
    StateReaderAndWriter<User>
    // + StateReader<Kernel> TODO: <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/596>
    + StateWriter<Accessory>
    + EventContainer
    + GasTracker<S>
{
}

/// A storage reader and writer which can access a particular namespace.
pub trait StateReaderAndWriter<N: CompileTimeNamespace>: StateReader<N> + StateWriter<N> {
    /// Removes a storage value and returns it
    fn remove(&mut self, key: &SlotKey) -> Option<SlotValue> {
        let value = self.get(key);
        self.delete(key);
        value
    }

    /// Removes a value from storage and decode the result
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

impl<T, N> StateReaderAndWriter<N> for T
where
    T: StateReader<N> + StateWriter<N>,
    N: CompileTimeNamespace,
{
}

/// A storage reader which can access a particular namespace.
pub trait StateReader<N: CompileTimeNamespace> {
    /// Get a value from the storage.
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue>;

    /// Get a decoded value from the storage.
    fn get_decoded<V, Codec>(&mut self, storage_key: &SlotKey, codec: &Codec) -> Option<V>
    where
        Codec: StateCodec,
        Codec::ValueCodec: StateItemCodec<V>,
    {
        let storage_value = self.get(storage_key)?;

        Some(codec.value_codec().decode_unwrap(storage_value.value()))
    }
}

/// Provides write-only access to a particular namespace
pub trait StateWriter<N: CompileTimeNamespace> {
    /// Replaces a storage value.
    fn set(&mut self, key: &SlotKey, value: SlotValue);

    /// Deletes a storage value.
    fn delete(&mut self, key: &SlotKey);
}

/// A helper trait allowing a type to access any namespace by their *runtime* enum variant.
pub(crate) trait UniversalStateAccessor {
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

/// Accepts events emitted by modules
pub trait EventContainer {
    /// Adds a typed event to the working set.
    fn add_event<E: 'static + core::marker::Send>(&mut self, event_key: &str, event: E);
}

/// Tracks gas usage.
pub trait GasTracker<S: Spec> {
    /// Attempts to charge the provided gas unit from the gas meter, using the internal price to
    /// compute the scalar value.
    fn charge_gas(&mut self, gas: &S::Gas) -> anyhow::Result<()>;
}
