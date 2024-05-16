//! Storage and state management interfaces for Sovereign SDK modules.

#![deny(missing_docs)]

mod bytes;
mod cache;
pub mod codec;
pub mod config;
pub mod namespaces;
#[cfg(feature = "native")]
mod prover_storage;
pub mod storage;
/// Defines the data structures needed by both the zk-storage and the prover storage.
mod storage_internals;
mod witness;
mod zk_storage;

pub mod jmt {
    //! Re-export the [`jellyfish-merkle-tree`](https://github.com/penumbra-zone/jmt) crate.
    pub use jmt::{KeyHash, RootHash, Version};
}
#[cfg(feature = "native")]
pub use prover_storage::{ProverChangeSet, ProverStorage};
use sha2::digest::typenum::U32;
use sov_rollup_interface::digest::Digest;
pub use storage_internals::{SparseMerkleProof, StorageRoot, VisibleHash};
pub use zk_storage::ZkStorage;

pub use crate::bytes::*;
pub use crate::cache::*;
pub use crate::codec::*;
pub use crate::namespaces::*;
pub use crate::storage::*;
pub use crate::witness::{ArrayWitness, Witness};

/// A trait specifying the hash function and format of the witness used in
/// merkle proofs for storage access
pub trait MerkleProofSpec: Send + Sync {
    /// The structure that accumulates the witness data
    type Witness: Witness + Send + Sync;
    /// The hash function used to compute the merkle root
    type Hasher: Digest<OutputSize = sha2::digest::typenum::U32> + Send + Sync;
}

/// The default [`MerkleProofSpec`] implementation.
///
/// This type is typically found as a type parameter for [`ProverStorage`].
#[derive(Clone)]
pub struct DefaultStorageSpec<H: Digest<OutputSize = U32> + Send + Sync> {
    _marker: std::marker::PhantomData<H>,
}

impl<H: Digest<OutputSize = U32> + Send + Sync> MerkleProofSpec for DefaultStorageSpec<H> {
    type Witness = ArrayWitness;

    type Hasher = H;
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
