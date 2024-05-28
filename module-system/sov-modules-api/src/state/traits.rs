use sov_state::{
    namespaces, Accessory, CompileTimeNamespace, EventContainer, SlotKey, SlotValue, StateCodec,
    StateItemCodec, StateItemDecoder, User,
};
#[cfg(feature = "native")]
use sov_state::{ProvableCompileTimeNamespace, StorageProof};

use super::accessors::seal::CachedAccessor;
use crate::{GasMeter, Spec};

/// The state accessor used during transaction execution. It provides unrestricted
/// access to [`User`]-space state, as well as limited visibility into the `Kernel` state.
pub trait TxState<S: Spec>:
    StateReaderAndWriter<User>
    // + StateReader<Kernel> TODO: <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/596>
    + StateWriter<Accessory>
    + EventContainer
    + GasMeter<S::Gas>
{
}

impl<S: Spec, T> TxState<S> for T where
    T: StateReaderAndWriter<User> + StateWriter<Accessory> + EventContainer + GasMeter<S::Gas>
{
}

/// The state accessor used during genesis. It provides unrestricted
/// access to [`User`] and `Kernel` state, as well as limited visibility into [`Accessory`] state.  
pub trait GenesisState<S: Spec>:
    StateReaderAndWriter<User>
    // + StateReader<Kernel> TODO: <https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/596>
    + StateWriter<Accessory>
    + EventContainer
    + GasMeter<S::Gas>
{}

impl<S: Spec, T> GenesisState<S> for T where
    T: StateReaderAndWriter<User>
        // + StateReaderAndWriter<sov_state::Kernel>
        + StateWriter<Accessory>
        + EventContainer
        + GasMeter<S::Gas>
{
}

/// A storage reader and writer which can access a particular namespace.
/// Does not charge gas for read/write operations.
pub trait StateReaderAndWriter<N: CompileTimeNamespace>: StateReader<N> + StateWriter<N> {
    /// Removes a storage value and returns it
    fn remove(&mut self, key: &SlotKey) -> Option<SlotValue> {
        let value = <Self as StateReader<N>>::get(self, key);
        <Self as StateWriter<N>>::delete(self, key);
        value
    }

    /// Removes a value from storage and decode the result
    fn remove_decoded<V, Codec>(&mut self, key: &SlotKey, codec: &Codec) -> Option<V>
    where
        Codec: StateCodec,
        Codec::ValueCodec: StateItemCodec<V>,
    {
        let value = self.get_decoded(key, codec);
        <Self as StateWriter<N>>::delete(self, key);
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
/// Does not charge gas for read operations.
pub trait StateReader<N: CompileTimeNamespace>: CachedAccessor<N> {
    /// Get a value from the storage.
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        <Self as CachedAccessor<N>>::get_cached(self, key).0
    }

    /// Get a decoded value from the storage.
    fn get_decoded<V, Codec>(&mut self, storage_key: &SlotKey, codec: &Codec) -> Option<V>
    where
        Codec: StateCodec,
        Codec::ValueCodec: StateItemCodec<V>,
    {
        let storage_value = <Self as StateReader<N>>::get(self, storage_key);

        storage_value.map(|storage_value| codec.value_codec().decode_unwrap(storage_value.value()))
    }
}

/// Provides write-only access to a particular namespace
/// Does not charge gas for write/delete operations.
pub trait StateWriter<N: CompileTimeNamespace>: CachedAccessor<N> {
    /// Replaces a storage value.
    fn set(&mut self, key: &SlotKey, value: SlotValue) {
        <Self as CachedAccessor<N>>::set_cached(self, key, value);
    }

    /// Deletes a storage value.
    fn delete(&mut self, key: &SlotKey) {
        <Self as CachedAccessor<N>>::delete_cached(self, key);
    }
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

/// A trait indicating that this working set is version aware
pub trait VersionReader: StateReaderAndWriter<namespaces::Kernel> {
    /// Returns the current version of the working set
    fn current_version(&self) -> u64;
}
