//! Bytes prefix definition.

use core::{fmt, str};

/// A prefix prepended to each key before insertion and retrieval from the storage.
///
/// When interacting with state containers, you will usually use the same working set instance to
/// access them, as required by the module API. This also means that you might get key collisions,
/// so it becomes necessary to prepend a prefix to each key.
#[derive(
    Debug,
    PartialEq,
    Eq,
    Clone,
    serde::Serialize,
    serde::Deserialize,
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
)]
#[cfg_attr(
    feature = "arbitrary",
    derive(arbitrary::Arbitrary, proptest_derive::Arbitrary)
)]
pub struct Prefix {
    prefix: Vec<u8>,
}

impl AsRef<[u8]> for Prefix {
    fn as_ref(&self) -> &[u8] {
        self.prefix.as_ref()
    }
}

impl fmt::Display for Prefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let buf = self.prefix.as_ref();
        match str::from_utf8(buf) {
            Ok(s) => {
                write!(f, "{:?}", s)
            }
            Err(_) => {
                write!(f, "0x{}", hex::encode(buf))
            }
        }
    }
}

impl Extend<u8> for Prefix {
    fn extend<T: IntoIterator<Item = u8>>(&mut self, iter: T) {
        self.prefix.extend(iter);
    }
}

impl Prefix {
    /// Creates a new prefix from a byte vector.
    pub fn new(prefix: Vec<u8>) -> Self {
        Self { prefix }
    }

    /// Creates a new prefix by extending an existing one with additional bytes.
    /// This method is particularly useful for the creation of nested sov containers.
    ///
    /// Caution: This method does not validate prefix collisions in the state tree. It is the caller's responsibility to ensure
    /// that the resulting prefix is unique.
    pub fn with_parent(parent_prefix: &Self, extra_prefix: &dyn AsRef<[u8]>) -> Self {
        let mut new_prefix = parent_prefix.as_ref().to_vec();
        new_prefix.extend_from_slice(extra_prefix.as_ref());
        Self::new(new_prefix)
    }

    /// Returns the length in bytes of the prefix.
    pub fn len(&self) -> usize {
        self.prefix.len()
    }

    /// Returns `true` if the prefix is empty, `false` otherwise.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.prefix.is_empty()
    }

    /// Returns a new prefix allocated on the fly, by extending the current
    /// prefix with the given bytes.
    pub fn extended(&self, bytes: &[u8]) -> Self {
        let mut prefix = self.clone();
        prefix.extend(bytes.iter().copied());
        prefix
    }
}
