//! Module storage definitions.

use alloc::vec::Vec;
use core::fmt;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sov_rollup_interface::maybestd::{vec, RefCount};

use crate::common::{AlignedVec, Prefix, Version, Witness};

mod cache;
mod codec;
mod scratchpad;

pub use cache::*;
pub use codec::*;
pub use scratchpad::*;

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
/// The namespaces used in the rollup. Related to the db's namespaces.
pub enum Namespace {
    /// The user namespace. Used by the User modules and is synchronised with the visible height.
    User,
    /// The kernel namespace. Used by the Kernel modules and is synchronised with the true height.
    Kernel,
}

#[derive(Default)]
/// A generic structure that divides a given type among the namespaces.
pub struct Namespaced<T> {
    user: T,
    kernel: T,
}

impl<T> Namespaced<T> {
    /// Gets the inner object for a given namespace.
    pub fn get(&self, namespace: Namespace) -> &T {
        match namespace {
            Namespace::User => &self.user,
            Namespace::Kernel => &self.kernel,
        }
    }

    /// Sets the inner object for a given namespace.
    pub fn set(&mut self, namespace: Namespace, state_update: T) {
        match namespace {
            Namespace::User => self.user = state_update,
            Namespace::Kernel => self.kernel = state_update,
        }
    }

    /// Creates a new struct instance from specified values.
    pub fn new(user: T, kernel: T) -> Self {
        Self { user, kernel }
    }
}

impl<T> From<Namespaced<T>> for (T, T) {
    fn from(val: Namespaced<T>) -> Self {
        (val.user, val.kernel)
    }
}

/// The key type suitable for use in [`Storage::get`] and other getter methods of
/// [`Storage`]. Cheaply-clonable.
#[derive(Clone, PartialEq, Eq, Debug, Hash, Ord, PartialOrd)]
#[cfg_attr(
    feature = "sync",
    derive(Serialize, serde::Deserialize, BorshDeserialize, BorshSerialize)
)]
pub struct SlotKey {
    namespace: Namespace,
    key: RefCount<Vec<u8>>,
}

impl SlotKey {
    /// Returns the namespace of the key.
    pub fn namespace(&self) -> Namespace {
        self.namespace
    }

    /// Returns a new [`RefCount`] reference to the bytes of this key.
    pub fn key(&self) -> RefCount<Vec<u8>> {
        self.key.clone()
    }

    /// Returns a new [`RefCount`] reference to the bytes of this key.
    pub fn key_ref(&self) -> &Vec<u8> {
        self.key.as_ref()
    }

    /// Converts this key into a [`SlotKey`] via cloning.
    pub fn to_cache_key_version(&self, version: Option<u64>) -> SlotKey {
        match version {
            None => SlotKey {
                namespace: self.namespace,
                key: self.key.clone(),
            },
            Some(v) => {
                let mut bytes = v.to_be_bytes().to_vec();
                bytes.extend((*self.key).clone());
                SlotKey {
                    namespace: self.namespace,
                    key: RefCount::new(bytes),
                }
            }
        }
    }
}

impl AsRef<Vec<u8>> for SlotKey {
    fn as_ref(&self) -> &Vec<u8> {
        &self.key
    }
}

impl fmt::Display for SlotKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:x?}", hex::encode(self.key().as_ref()))
    }
}

impl SlotKey {
    /// Creates a new [`SlotKey`] that combines a prefix and a key.
    pub fn new<K, Q, KC>(namespace: Namespace, prefix: &Prefix, key: &Q, codec: &KC) -> Self
    where
        KC: EncodeKeyLike<Q, K>,
        Q: ?Sized,
    {
        let encoded_key = codec.encode_key_like(key);
        let encoded_key = AlignedVec::new(encoded_key);

        let full_key = Vec::<u8>::with_capacity(prefix.len() + encoded_key.len());
        let mut full_key = AlignedVec::new(full_key);
        full_key.extend(prefix.as_aligned_vec());
        full_key.extend(&encoded_key);

        Self {
            namespace,
            key: RefCount::new(full_key.into_inner()),
        }
    }

    /// Build a storage key from raw bytes
    pub fn from_bytes(namespace: Namespace, bytes: Vec<u8>) -> Self {
        Self {
            namespace,
            key: RefCount::new(bytes),
        }
    }

    /// Used only in tests.
    /// Builds a storage key from a string
    pub fn from_str(namespace: Namespace, key: &str) -> Self {
        Self {
            namespace,
            key: RefCount::new(key.as_bytes().to_vec()),
        }
    }

    /// Creates a new [`SlotKey`] that combines a prefix and a key.
    pub fn singleton(namespace: Namespace, prefix: &Prefix) -> Self {
        Self {
            namespace,
            key: RefCount::new(prefix.as_aligned_vec().clone().into_inner()),
        }
    }
}

/// A serialized value suitable for storing. Internally uses an [`RefCount<Vec<u8>>`]
/// for cheap cloning.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
#[cfg_attr(
    feature = "sync",
    derive(Serialize, serde::Deserialize, BorshDeserialize, BorshSerialize)
)]
pub struct SlotValue {
    value: RefCount<Vec<u8>>,
}

impl From<Vec<u8>> for SlotValue {
    fn from(value: Vec<u8>) -> Self {
        Self {
            value: RefCount::new(value),
        }
    }
}

impl SlotValue {
    /// Create a new storage value by serializing the input with the given codec.
    pub fn new<V, VC>(value: &V, codec: &VC) -> Self
    where
        VC: StateValueCodec<V>,
    {
        let encoded_value = codec.encode_value(value);
        Self {
            value: RefCount::new(encoded_value),
        }
    }

    /// Get the bytes of this value.
    pub fn value(&self) -> &[u8] {
        &self.value
    }

    /// Returns the value as a vector of bytes.
    pub fn value_as_vec(self) -> Vec<u8> {
        RefCount::<vec::Vec<u8>>::try_unwrap(self.value)
            .expect("Impossible to unwrap the storage value")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    feature = "sync",
    derive(Serialize, serde::Deserialize, BorshDeserialize, BorshSerialize)
)]
/// A proof that a particular storage key has a particular value, or is absent.
pub struct StorageProof<P> {
    /// The key which is proven
    pub key: SlotKey,
    /// The value, if any, which is proven
    pub value: Option<SlotValue>,
    /// The cryptographic proof
    pub proof: P,
}

/// An interface for storing and retrieving values in the storage.
pub trait Storage: Clone {
    /// The witness type for this storage instance.
    type Witness: Witness + Send + Sync;

    /// The runtime config for this storage instance.
    type RuntimeConfig;

    /// A cryptographic proof that a particular key has a particular value, or is absent.
    type Proof: Serialize
        + DeserializeOwned
        + fmt::Debug
        + Clone
        + BorshSerialize
        + BorshDeserialize;

    /// A cryptographic commitment to the contents of this storage
    type Root: Serialize
        + DeserializeOwned
        + fmt::Debug
        + Clone
        + BorshSerialize
        + BorshDeserialize
        + Eq
        + Send
        + Sync
        + AsRef<[u8]>
        + Into<[u8; 32]>; // Require a one-way conversion from the state root to a 32-byte array. This can always be
                          // implemented by hashing the state root even if the root itself is not 32 bytes.

    /// State update that will be committed to the database.
    type StateUpdate;

    /// Collections of all the writes that have been made on top of this instance of the storage;
    type ChangeSet;

    /// Returns the value corresponding to the key or None if key is absent.
    fn get(
        &self,
        key: &SlotKey,
        version: Option<Version>,
        witness: &Self::Witness,
    ) -> Option<SlotValue>;

    /// Returns the value corresponding to the key or None if key is absent.
    ///
    /// # About accessory state
    /// This method is blanket-implemented to return [`None`]. **Only native
    /// execution environments** (i.e. outside of the zmVM) **SHOULD** override
    /// this method to return a value. This is because accessory state **MUST
    /// NOT** be readable from within the zmVM.
    fn get_accessory(&self, _key: &SlotKey, _version: Option<Version>) -> Option<SlotValue> {
        None
    }

    /// Calculates new state root but does not commit any changes to the database.
    fn compute_state_update(
        &self,
        state_accesses: OrderedReadsAndWrites,
        witness: &Self::Witness,
    ) -> Result<(Self::Root, Self::StateUpdate), anyhow::Error>;

    /// Commits state changes to the underlying storage.
    fn commit(&self, node_batch: &Self::StateUpdate, accessory_update: &OrderedReadsAndWrites);

    /// A version of [`Storage::validate_and_commit`] that allows for "accessory" non-JMT updates.
    fn validate_and_commit_with_accessory_update(
        &self,
        state_accesses: OrderedReadsAndWrites,
        witness: &Self::Witness,
        accessory_update: &OrderedReadsAndWrites,
    ) -> Result<Self::Root, anyhow::Error> {
        let (root_hash, node_batch) = self.compute_state_update(state_accesses, witness)?;
        self.commit(&node_batch, accessory_update);

        Ok(root_hash)
    }

    /// Validate all of the storage accesses in a particular cache log,
    /// returning the new state root after applying all writes.
    /// This function is equivalent to calling:
    /// `self.compute_state_update & self.commit`
    fn validate_and_commit(
        &self,
        state_accesses: OrderedReadsAndWrites,
        witness: &Self::Witness,
    ) -> Result<Self::Root, anyhow::Error> {
        Self::validate_and_commit_with_accessory_update(
            self,
            state_accesses,
            witness,
            &Default::default(),
        )
    }

    /// Opens a storage access proof and validates it against a state root.
    /// It returns a result with the opened leaf (key, value) pair in case of success.
    fn open_proof(
        state_root: Self::Root,
        proof: StorageProof<Self::Proof>,
    ) -> Result<(SlotKey, Option<SlotValue>), anyhow::Error>;

    /// Indicates if storage is empty or not.
    /// Useful during initialization.
    fn is_empty(&self) -> bool;

    /// Converts the storage into a change set.
    fn to_change_set(self) -> Self::ChangeSet;
}

/// Used only in tests.
impl From<&str> for SlotValue {
    fn from(value: &str) -> Self {
        Self {
            value: RefCount::new(value.as_bytes().to_vec()),
        }
    }
}

/// A [`Storage`] that is suitable for use in native execution environments
/// (outside of the zkVM).
pub trait NativeStorage: Storage {
    /// Returns the value corresponding to the key or None if key is absent and a proof to
    /// get the value.
    fn get_with_proof(&self, key: SlotKey) -> StorageProof<Self::Proof>;

    /// Get the *global* root hash of the tree at the requested version
    fn get_root_hash(&self, version: Version) -> Result<Self::Root, anyhow::Error>;
}
