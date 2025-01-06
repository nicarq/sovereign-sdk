//! Module storage definitions.

use core::fmt;
use std::sync::Arc;

use borsh::{BorshDeserialize, BorshSerialize};
use derivative::Derivative;
use serde::de::DeserializeOwned;
use serde::Serialize;
#[cfg(feature = "native")]
use sov_rollup_interface::sov_universal_wallet::UniversalWallet;

use crate::bytes::Prefix;
use crate::codec::EncodeLike;
use crate::jmt::Version;
use crate::namespaces::{ProvableCompileTimeNamespace, ProvableNamespace};
use crate::{StateAccesses, StateItemDecoder, Witness};

type ArcFormatFn =
    Arc<dyn (Fn(&[u8], &mut fmt::Formatter<'_>) -> fmt::Result) + Send + Sync + 'static>;

/// The key type suitable for use in [`Storage::get`] and other getter methods of
/// [`Storage`]. Cheaply-clonable.
#[derive(Derivative, Serialize, serde::Deserialize, BorshDeserialize, BorshSerialize)]
#[derivative(Clone, PartialEq, Eq, Debug, Hash, Ord)]
#[cfg_attr(feature = "native", derive(UniversalWallet))]
pub struct SlotKey {
    #[cfg_attr(feature = "native", sov_wallet(hidden))]
    key: Arc<Vec<u8>>,
    #[borsh(skip)]
    #[serde(skip)]
    #[derivative(
        Debug = "ignore",
        PartialEq = "ignore",
        Hash = "ignore",
        Ord = "ignore"
    )]
    #[cfg_attr(feature = "native", sov_wallet(skip))]
    display_fn: Option<ArcFormatFn>,
}

// Manually implement PartialOrd to satisfy clippy
impl PartialOrd for SlotKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl From<Vec<u8>> for SlotKey {
    fn from(key: Vec<u8>) -> Self {
        Self {
            key: Arc::new(key),
            display_fn: None,
        }
    }
}

impl SlotKey {
    /// Returns a new [`Arc`] reference to the bytes of this key.
    pub fn key(&self) -> Arc<Vec<u8>> {
        self.key.clone()
    }

    /// Returns a new [`Arc`] reference to the bytes of this key.
    pub fn key_ref(&self) -> &Vec<u8> {
        self.key.as_ref()
    }
}

impl AsRef<Vec<u8>> for SlotKey {
    fn as_ref(&self) -> &Vec<u8> {
        &self.key
    }
}

impl fmt::Display for SlotKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(display_fn) = &self.display_fn {
            display_fn(self.key.as_ref(), f)
        } else {
            write!(f, "{}", String::from_utf8_lossy(self.key().as_ref()))
        }
    }
}

impl SlotKey {
    /// Creates a new [`SlotKey`] that combines a prefix and a key.
    pub fn new<K, Q, KC>(prefix: &Prefix, key: &Q, codec: &KC) -> Self
    where
        KC: EncodeLike<Q, K> + StateItemDecoder<K> + 'static,
        K: fmt::Display,
        Q: ?Sized,
    {
        let encoded_key = codec.encode_like(key);

        let mut full_key = Vec::<u8>::with_capacity(prefix.len().saturating_add(encoded_key.len()));
        full_key.extend(prefix.as_ref());
        full_key.extend(&encoded_key);
        let prefix_len = prefix.len();
        let codec = codec.clone();
        let display_fn: Option<ArcFormatFn> = Some(Arc::new(
            move |key_bytes: &[u8], formatter: &mut fmt::Formatter<'_>| {
                if key_bytes.len() < prefix_len {
                    return Err(std::fmt::Error);
                }
                let prefix = &key_bytes[..prefix_len];
                let key = &key_bytes[prefix_len..];
                let key = KC::try_decode(&codec, key).map_err(|_| std::fmt::Error)?;
                let prefix_str = std::str::from_utf8(prefix).map_err(|_e| std::fmt::Error)?;
                write!(formatter, "{}{}", prefix_str, key)
            },
        ));
        Self {
            key: Arc::new(full_key),
            display_fn,
        }
    }

    /// Used only in tests.
    /// Builds a storage key from a byte slice
    pub fn from_slice(key: &[u8]) -> Self {
        Self {
            key: Arc::new(key.to_vec()),
            display_fn: None,
        }
    }

    /// Creates a new [`SlotKey`] from a prefix.
    pub fn singleton(prefix: &Prefix) -> Self {
        Self {
            key: Arc::new(prefix.as_ref().to_vec()),
            display_fn: Some(Arc::new(
                move |key_bytes: &[u8], formatter: &mut fmt::Formatter<'_>| {
                    let prefix_str =
                        std::str::from_utf8(key_bytes).map_err(|_e| std::fmt::Error)?;
                    formatter.write_str(prefix_str)
                },
            )),
        }
    }
}

/// A serialized value suitable for storing. Internally uses an [`Arc<Vec<u8>>`]
/// for cheap cloning.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Default,
    Serialize,
    serde::Deserialize,
    BorshDeserialize,
    BorshSerialize,
)]
#[cfg_attr(feature = "native", derive(UniversalWallet))]
pub struct SlotValue {
    #[cfg_attr(feature = "native", sov_wallet(hidden))]
    value: Arc<Vec<u8>>,
}

impl From<Vec<u8>> for SlotValue {
    fn from(value: Vec<u8>) -> Self {
        Self {
            value: Arc::new(value),
        }
    }
}

impl SlotValue {
    /// Create a new storage value by serializing the input with the given codec.
    pub fn new<V, Vq, VC>(value: &Vq, codec: &VC) -> Self
    where
        Vq: ?Sized,
        VC: EncodeLike<Vq, V>,
    {
        let encoded_value = codec.encode_like(value);
        Self {
            value: Arc::new(encoded_value),
        }
    }

    /// Get the bytes of this value.
    pub fn value(&self) -> &[u8] {
        &self.value
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize, BorshDeserialize, BorshSerialize,
)]
#[cfg_attr(feature = "native", derive(UniversalWallet))]
/// A proof that a particular storage key has a particular value, or is absent.
// Note: This type intentionally does not derive `UniveralWallet` because the slotkey and slotvalue
// can't be displayed meaningfully without additional context
pub struct StorageProof<P> {
    /// The key which is proven
    pub key: SlotKey,
    /// The value, if any, which is proven
    pub value: Option<SlotValue>,
    /// The cryptographic proof
    #[cfg_attr(feature = "native", sov_wallet(hidden))]
    pub proof: P,
    /// The namespace of the key.
    pub namespace: ProvableNamespace,
}

/// A trait implemented by state updates that can be committed to the database.
pub trait StateUpdate {
    /// Adds a non-provable ("accessory") state change to the
    /// state update after the rest of the update is finalized.
    fn add_accessory_item(&mut self, key: SlotKey, value: Option<SlotValue>);

    /// Adds a collection of non-provable ("accessory") state changes to the
    /// state update after the rest of the update is finalized.
    fn add_accessory_items(&mut self, items: Vec<(SlotKey, Option<SlotValue>)>) {
        for (key, value) in items {
            self.add_accessory_item(key, value);
        }
    }
}

impl StateUpdate for () {
    fn add_accessory_item(&mut self, _key: SlotKey, _value: Option<SlotValue>) {
        // Silently discard the input. This is safe, since the accessory state
        // is *not* consensus critical. This implementation is intended to be used
        // in the zk context only. In the native context, a real implementation SHOULD
        // be used instead.
    }
}

/// A trait that represents the root hash of a state tree.
pub trait StateRoot:
    Serialize
    + DeserializeOwned
    + fmt::Debug
    + Clone
    + BorshSerialize
    + BorshDeserialize
    + Eq
    + Send
    + Sync
    + AsRef<[u8]>
{
    /// Gets the global root hash of the storage. Ie, the root hash of the entire tree for all namespaces.
    /// We always require a one-way conversion from the state root to a 32-byte array. This can be
    /// implemented by hashing the state root even if the root itself is not 32 bytes.
    fn global_root(&self) -> [u8; 32];

    /// Gets the root hash of a specific namespace
    fn namespace_root(&self, namespace: ProvableNamespace) -> [u8; 32];

    /// Builds a storage root from underlying namespace roots.
    fn from_namespace_roots(user_root: [u8; 32], kernel_root: [u8; 32]) -> Self;
}

/// An interface for retrieving values from the storage and producing change set of new write operations.
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
        + BorshDeserialize
        + Send
        + Sync
        + PartialEq
        + Eq;

    /// A cryptographic commitment to the contents of this storage.
    type Root: StateRoot;

    /// State update that will be committed to the database.
    type StateUpdate: StateUpdate;

    /// Collections of all the writes that have been made on top of this instance of the storage;
    type ChangeSet: Send + Sync;

    /// Returns the value corresponding to the key or None if key is absent.
    fn get<N: ProvableCompileTimeNamespace>(
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
        state_accesses: StateAccesses,
        witness: &Self::Witness,
    ) -> anyhow::Result<(Self::Root, Self::StateUpdate)>;

    /// Materializes changes from given [`Self::StateUpdate`] into [`Self::ChangeSet`].
    fn materialize_changes(&self, state_update: &Self::StateUpdate) -> Self::ChangeSet;

    /// A version of [`Storage::validate_and_materialize`] that allows for "accessory" non-JMT updates.
    fn validate_and_materialize_with_accessory_update(
        &self,
        state_accesses: StateAccesses,
        witness: &Self::Witness,
        accessory_updates: Vec<(SlotKey, Option<SlotValue>)>,
    ) -> anyhow::Result<(Self::Root, Self::ChangeSet)> {
        let (root_hash, mut node_batch) = self.compute_state_update(state_accesses, witness)?;
        for write in accessory_updates {
            node_batch.add_accessory_item(write.0, write.1);
        }
        let change_set = self.materialize_changes(&node_batch);

        Ok((root_hash, change_set))
    }

    /// Validate all the storage accesses in a particular cache log,
    /// returning the new state root and change set after applying all writes.
    /// This function is equivalent to calling:
    /// `self.compute_state_update` & `self.materialize_changes`
    fn validate_and_materialize(
        &self,
        state_accesses: StateAccesses,
        witness: &Self::Witness,
    ) -> anyhow::Result<(Self::Root, Self::ChangeSet)> {
        Self::validate_and_materialize_with_accessory_update(
            self,
            state_accesses,
            witness,
            Default::default(),
        )
    }

    /// Opens a storage access proof and validates it against a state root.
    /// It returns a result with the opened leaf (key, value) pair in case of success.
    fn open_proof(
        state_root: Self::Root,
        proof: StorageProof<Self::Proof>,
    ) -> anyhow::Result<(SlotKey, Option<SlotValue>)>;
}

/// Used only in tests.
impl From<&str> for SlotValue {
    fn from(value: &str) -> Self {
        Self {
            value: Arc::new(value.as_bytes().to_vec()),
        }
    }
}

/// A [`Storage`] that is suitable for use in native execution environments
/// (outside of the zkVM).
pub trait NativeStorage: Storage {
    /// Returns the value corresponding to the key or None if the key is absent and a proof to
    /// get the value.
    /// Returns an error if storage is empty or the passed version is not yet available.
    fn get_with_proof<N: ProvableCompileTimeNamespace>(
        &self,
        key: SlotKey,
        version: Option<u64>,
    ) -> anyhow::Result<StorageProof<Self::Proof>>;

    /// Get the *global* root hash of the tree at the requested version.
    /// Returns an error if storage is empty or the requests version is not yet available.
    fn get_root_hash(&self, version: Version) -> anyhow::Result<Self::Root>;
}
