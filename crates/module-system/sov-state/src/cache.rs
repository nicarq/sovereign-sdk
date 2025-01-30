//! Cache key/value definitions

use std::collections::hash_map::Entry;

use sov_rollup_interface::common::SlotNumber;

use crate::namespaces::ProvableCompileTimeNamespace;
use crate::storage::{SlotKey, SlotValue, Storage};

/// An enum that represents the temperature of a value in the storage.
/// Used in cached-structs to determine whether this is the first read of a value or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IsValueCached {
    /// The value is cached.
    Yes,
    /// The value is fetched from the storage and was never cached.
    No,
}

/// An error when reading from the cache.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum ReadError {
    /// The value returned from the cache is not expected.
    #[error("inconsistent read, expected: {expected:?}, found: {found:?}")]
    InconsistentRead {
        /// Expected value.
        expected: Option<SlotValue>,
        /// Found value.
        found: Option<SlotValue>,
    },
}

/// `Access` represents a sequence of events on a particular value.
/// For example, a transaction might read a value, then take some action which causes it to be updated
/// The rules for defining causality are as follows:
/// 1. If a read is preceded by another read, check that the two reads match and discard one.
/// 2. If a read is preceded by a write, check that the value read matches the value written. Discard the read.
/// 3. Otherwise, retain the read.
/// 4. A write is retained unless it is followed by another write.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Access {
    Read {
        original: Option<SlotValue>,
    },
    Write {
        modified: Option<SlotValue>,
    },
    ReadThenWrite {
        original: Option<SlotValue>,
        modified: Option<SlotValue>,
    },
}

impl Access {
    fn original(&self) -> Option<Option<&SlotValue>> {
        match self {
            Access::Write { .. } => None,
            Access::Read { original } | Access::ReadThenWrite { original, .. } => {
                Some(original.as_ref())
            }
        }
    }

    fn modified(&self) -> Option<Option<&SlotValue>> {
        match self {
            Access::Read { .. } => None,
            Access::ReadThenWrite { modified, .. } | Access::Write { modified } => {
                Some(modified.as_ref())
            }
        }
    }

    fn modified_mut(&mut self) -> Option<&mut Option<SlotValue>> {
        match self {
            Access::Read { .. } => None,
            Access::ReadThenWrite { modified, .. } | Access::Write { modified } => Some(modified),
        }
    }

    fn last_value(&self) -> Option<&SlotValue> {
        // `.unwrap()`: if a write doesn't exist, then it's guaranteed to be a
        // `Read`.
        self.modified().unwrap_or_else(|| self.original().unwrap())
    }

    fn add_write(&mut self, write: Option<SlotValue>) {
        match self {
            Access::Read { original } => {
                // A read with a new modified value becomes a `ReadThenWrite`.
                // We add the write only if the new value is different from the
                // original value.
                if original != &write {
                    *self = Access::ReadThenWrite {
                        original: original.take(),
                        modified: write,
                    };
                }
            }
            Access::ReadThenWrite { modified, .. } | Access::Write { modified } => {
                // Simply override the modified value with the new modified
                // value.
                *modified = write;
            }
        }
    }
}

/// Cache entry can be in three states:
/// - Does not exists, a given key was never inserted in the cache:
///     ValueExists::No
/// - Exists but the value is empty.
///      ValueExists::Yes(None)
/// - Exists and contains a value:
///     ValueExists::Yes(Some(value))
enum ValueExistsInCache {
    /// The key exists in the cache.
    Yes(Option<SlotValue>),
    /// The key does not exist in the cache.
    No,
}

/// CacheLog keeps track of the original and current values of each key accessed.
/// By tracking original values, we can detect and eliminate write patterns where a key is
/// changed temporarily and then reset to its original value
#[derive(Default, Debug, Clone)]
pub struct CacheLog {
    log: std::collections::HashMap<SlotKey, Access>,
}

impl CacheLog {
    /// Returns the owned set of key/value pairs of the cache.
    pub fn take_writes(self) -> Vec<(SlotKey, Option<SlotValue>)> {
        self.log
            .into_iter()
            .filter_map(|(k, mut access)| access.modified_mut().map(|value| (k, value.take())))
            .collect()
    }

    /// Returns the owned set of key/value pairs of the cache.
    pub fn get_writes(&self) -> impl Iterator<Item = (&SlotKey, Option<&SlotValue>)> {
        self.log
            .iter()
            .filter_map(|(k, access)| access.modified().map(|v| (k, v)))
    }

    /// Returns a value corresponding to the key.
    fn get_value(&self, key: &SlotKey) -> ValueExistsInCache {
        match self.log.get(key) {
            Some(value) => ValueExistsInCache::Yes(value.last_value().cloned()),
            None => ValueExistsInCache::No,
        }
    }

    /// The first read for a given key is inserted in the cache. For an existing cache entry
    /// checks if reads are consistent with previous reads/writes.
    pub fn add_read(&mut self, key: SlotKey, value: Option<SlotValue>) -> Result<(), ReadError> {
        if let Some(existing) = self.log.get(&key) {
            let last_value = existing.last_value().cloned();

            if last_value != value {
                return Err(ReadError::InconsistentRead {
                    expected: last_value,
                    found: value,
                });
            }
        } else {
            self.log.insert(key, Access::Read { original: value });
        }

        Ok(())
    }

    /// Adds a write entry to the cache.
    pub fn add_write(&mut self, key: SlotKey, value: Option<SlotValue>) -> IsValueCached {
        match self.log.entry(key) {
            Entry::Occupied(mut existing) => {
                existing.get_mut().add_write(value);
                IsValueCached::Yes
            }
            Entry::Vacant(vacancy) => {
                vacancy.insert(Access::Write { modified: value });
                IsValueCached::No
            }
        }
    }
}

/// Caches reads and writes for a (key, value) pair. On the first read the value is fetched
/// from an external source represented by the `ValueReader` trait. On following reads,
/// the cache checks if the value we read was inserted before.
#[derive(Default, Debug, Clone)]
pub struct ProvableStorageCache<N> {
    /// Transaction cache.
    pub tx_cache: CacheLog,
    /// Ordered reads and writes.
    pub ordered_db_reads: Vec<(SlotKey, Option<SlotValue>)>,
    phantom: core::marker::PhantomData<N>,
}

// We implement these methods only for *provable* state values because the internal cache
// does extra bookkeeping which is not useful for accessory state.
impl<N: ProvableCompileTimeNamespace> ProvableStorageCache<N> {
    /// Gets a value from the cache or reads it from the provided `ValueReader`.
    pub fn get_or_fetch<S: Storage>(
        &mut self,
        key: &SlotKey,
        value_reader: &S,
        witness: &S::Witness,
        version: Option<SlotNumber>,
    ) -> (Option<SlotValue>, IsValueCached) {
        let read = self.get_without_caching(key, value_reader, witness, version);
        if read.1 == IsValueCached::No {
            self.add_read(key.clone(), read.0.clone());
        }

        read
    }

    /// Like [`ProvableStorageCache::get_or_fetch`] but does not add the read to the cache.
    pub fn get_without_caching<S: Storage>(
        &self,
        key: &SlotKey,
        value_reader: &S,
        witness: &S::Witness,
        version: Option<SlotNumber>,
    ) -> (Option<SlotValue>, IsValueCached) {
        match self.tx_cache.get_value(key) {
            ValueExistsInCache::Yes(cache_value) => (cache_value, IsValueCached::Yes),
            // If the value does not exist in the cache, then fetch it from an external source.
            ValueExistsInCache::No => {
                let storage_value = value_reader.get::<N>(key, version, witness);
                (storage_value, IsValueCached::No)
            }
        }
    }

    /// Replaces the keyed value on the storage.
    pub fn set(&mut self, key: &SlotKey, value: SlotValue) -> IsValueCached {
        self.tx_cache.add_write(key.clone(), Some(value))
    }

    /// Deletes a keyed value from the cache.
    pub fn delete(&mut self, key: &SlotKey) -> IsValueCached {
        self.tx_cache.add_write(key.clone(), None)
    }

    fn add_read(&mut self, key: SlotKey, value: Option<SlotValue>) {
        self.tx_cache
            .add_read(key.clone(), value.clone())
            // It is ok to panic here, we must guarantee that the cache is consistent.
            .unwrap_or_else(|e| panic!("Inconsistent read from the cache: {e:?}"));
        self.ordered_db_reads.push((key, value));
    }
}

/// A struct that contains the values read from the DB and the values to be written, both in
/// deterministic order.
#[derive(Debug, Default)]
pub struct OrderedReadsAndWrites {
    /// Ordered reads.
    pub ordered_reads: Vec<(SlotKey, Option<SlotValue>)>,
    /// Ordered writes.
    pub ordered_writes: Vec<(SlotKey, Option<SlotValue>)>,
}

/// A struct that contains the read/write sets for the user and kernel namespaces.

#[derive(Debug, Default)]
pub struct StateAccesses {
    /// The reads and writes to the user namespace
    pub user: OrderedReadsAndWrites,
    /// The reads and writes to the user namespace
    pub kernel: OrderedReadsAndWrites,
}

impl<N> From<ProvableStorageCache<N>> for OrderedReadsAndWrites {
    fn from(val: ProvableStorageCache<N>) -> Self {
        let mut writes = val.tx_cache.take_writes();
        // TODO: Make this more efficient
        writes.sort_by(|(k1, _), (k2, _)| k1.cmp(k2));
        Self {
            ordered_reads: val.ordered_db_reads,
            ordered_writes: writes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Access {
        fn read(original: Option<SlotValue>) -> Self {
            Access::Read { original }
        }

        fn write(modified: Option<SlotValue>) -> Self {
            Access::Write { modified }
        }
    }

    pub fn create_key(key: u8) -> SlotKey {
        SlotKey::from(vec![key])
    }

    pub fn create_value(v: u8) -> Option<SlotValue> {
        Some(SlotValue::from(vec![v]))
    }

    impl ValueExistsInCache {
        fn get(self) -> Option<SlotValue> {
            match self {
                ValueExistsInCache::Yes(value) => value,
                ValueExistsInCache::No => unreachable!(),
            }
        }
    }

    #[test]
    fn test_cache_read_write() {
        let mut cache_log = CacheLog::default();
        let key = create_key(1);

        {
            let value = create_value(2);

            cache_log.add_read(key.clone(), value.clone()).unwrap();
            let value_from_cache = cache_log.get_value(&key).get();
            assert_eq!(value_from_cache, value);
        }

        {
            let value = create_value(3);

            cache_log.add_write(key.clone(), value.clone());

            let value_from_cache = cache_log.get_value(&key).get();
            assert_eq!(value_from_cache, value);

            cache_log.add_read(key.clone(), value.clone()).unwrap();

            let value_from_cache = cache_log.get_value(&key).get();
            assert_eq!(value_from_cache, value);
        }
    }

    #[derive(PartialEq, Eq, Clone, Debug)]
    pub(crate) struct CacheEntry {
        key: SlotKey,
        value: Option<SlotValue>,
    }

    impl CacheEntry {
        fn new(key: SlotKey, value: Option<SlotValue>) -> Self {
            Self { key, value }
        }
    }

    fn new_cache_entry(key: u8, value: u8) -> CacheEntry {
        CacheEntry::new(create_key(key), create_value(value))
    }

    #[test]
    fn test_add_read() {
        let mut cache = CacheLog::default();

        let entry = new_cache_entry(1, 1);

        let res = cache.add_read(entry.key, entry.value);
        assert!(res.is_ok());

        let entry = new_cache_entry(2, 1);
        let res = cache.add_read(entry.key, entry.value);
        assert!(res.is_ok());

        let entry = new_cache_entry(1, 2);
        let res = cache.add_read(entry.key, entry.value);

        assert_eq!(
            res,
            Err(ReadError::InconsistentRead {
                expected: create_value(1),
                found: create_value(2)
            })
        );
    }

    #[test]
    fn test_access_read_write() {
        let original_value = create_value(1);
        let mut access = Access::read(original_value.clone());

        // Check: Read => ReadThenWrite transition
        {
            let new_value = create_value(2);
            access.add_write(new_value.clone());

            assert_eq!(access.last_value(), new_value.as_ref());
            assert_eq!(
                access,
                Access::ReadThenWrite {
                    original: original_value.clone(),
                    modified: new_value
                }
            );
        }

        // Check: ReadThenWrite => ReadThenWrite transition
        {
            let new_value = create_value(3);
            access.add_write(new_value.clone());

            assert_eq!(access.last_value(), new_value.as_ref());
            assert_eq!(
                access,
                Access::ReadThenWrite {
                    original: original_value,
                    modified: new_value
                }
            );
        }
    }

    #[test]
    fn test_access_write() {
        let original_value = create_value(1);
        let mut access = Access::write(original_value.clone());

        // Check: Write => Write transition
        {
            assert_eq!(access.last_value(), original_value.as_ref());
            let new_value = create_value(3);
            access.add_write(new_value.clone());
            assert_eq!(access.last_value(), new_value.as_ref());
            assert_eq!(access, Access::write(new_value));
        }
    }
}
