//! Cache key/value definitions

use std::collections::hash_map::Entry;

use sov_rollup_interface::common::SlotNumber;

use crate::namespaces::ProvableCompileTimeNamespace;
use crate::storage::{SlotKey, SlotValue, Storage};
use crate::{NodeLeaf, NodeLeafAndValue};

/// An enum that represents the temperature of a value in the storage.
/// Used in cached-structs to determine whether this is the first read of a value or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IsValueCached {
    /// The value is cached.
    Yes,
    /// The value is fetched from the storage and was never cached.
    No,
}

/// `Access` represents a sequence of events on a particular value.
/// For example, a transaction might read a value, then take some action which causes it to be updated
#[derive(Debug, Clone, PartialEq, Eq)]
enum Access {
    Read { original: Option<NodeLeafAndValue> },
    Write { modified: Option<SlotValue> },
}

impl Access {
    fn modified(&self) -> Option<Option<&SlotValue>> {
        match self {
            Access::Read { .. } => None,
            Access::Write { modified } => Some(modified.as_ref()),
        }
    }

    fn modified_mut(&mut self) -> Option<&mut Option<SlotValue>> {
        match self {
            Access::Read { .. } => None,
            Access::Write { modified } => Some(modified),
        }
    }

    fn add_write(&mut self, write: Option<SlotValue>) {
        match self {
            Access::Read { original: _ } => *self = Access::Write { modified: write },
            Access::Write { modified } => {
                // Simply override the modified value with the new modified
                // value.
                *modified = write;
            }
        }
    }
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
    pub fn get_writes(&self) -> impl Iterator<Item = (&SlotKey, Option<&SlotValue>)> {
        self.log
            .iter()
            .filter_map(|(k, access)| access.modified().map(|v| (k, v)))
    }

    // Returns the owned set of key/value pairs of the cache.
    fn take_writes(self) -> Vec<(SlotKey, Option<SlotValue>)> {
        self.log
            .into_iter()
            .filter_map(|(k, mut access)| access.modified_mut().map(|value| (k, value.take())))
            .collect()
    }

    // Adds a read entry to the cache.
    fn add_read(&mut self, key: SlotKey, value: Option<NodeLeafAndValue>) {
        match self.log.entry(key) {
            Entry::Occupied(existing) => {
                // Sanity check.
                panic!(
                    "Detected multiple calls to `add_read` for the same key.: {:?}",
                    existing.key()
                );
            }
            Entry::Vacant(vacancy) => vacancy.insert(Access::Read { original: value }),
        };
    }

    // Adds a write entry to the cache.
    fn add_write(&mut self, key: SlotKey, value: Option<SlotValue>) -> IsValueCached {
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
    pub ordered_db_reads: Vec<(SlotKey, Option<NodeLeaf>)>,
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
        match self.tx_cache.log.get(key) {
            Some(access) => {
                let value = match access {
                    Access::Read { original } => original.clone().map(|node| node.value),
                    Access::Write { modified } => modified.clone(),
                };
                (value, IsValueCached::Yes)
            }
            None => {
                let storage_value = value_reader.get::<N>(key, version, witness);
                let read = storage_value
                    .clone()
                    .map(NodeLeafAndValue::new::<S::Hasher>);
                self.add_read(key.clone(), read);
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

    // This method can be called only once per given key.
    fn add_read(&mut self, key: SlotKey, node: Option<NodeLeafAndValue>) {
        self.ordered_db_reads
            .push((key.clone(), node.as_ref().map(|n| n.leaf)));

        self.tx_cache.add_read(key.clone(), node.clone());
    }
}

/// A struct that contains the values read from the DB and the values to be written, both in
/// deterministic order.
#[derive(Debug, Default)]
pub struct OrderedReadsAndWrites {
    /// Ordered reads.
    pub ordered_reads: Vec<(SlotKey, Option<NodeLeaf>)>,
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

    pub fn create_key(key: u8) -> SlotKey {
        SlotKey::from(vec![key])
    }

    pub fn create_value(v: u8) -> Option<SlotValue> {
        Some(SlotValue::from(vec![v]))
    }

    #[test]
    fn test_cache_read_write() {
        let key = create_key(1);

        // Test read.
        {
            let mut cache_log = CacheLog::default();
            let value = create_value(2).map(NodeLeafAndValue::new::<sha2::Sha256>);
            cache_log.add_read(key.clone(), value);

            let writes = cache_log.take_writes();
            assert_eq!(writes.len(), 0);
        }

        // Test write.
        {
            let mut cache_log = CacheLog::default();
            let value = create_value(3);
            cache_log.add_write(key.clone(), value.clone());

            let writes = cache_log.take_writes();
            assert_eq!(writes.len(), 1);
            assert_eq!((key.clone(), value), writes[0]);
        }

        // Test that write overrides read.
        {
            let mut cache_log = CacheLog::default();
            let value = create_value(4).map(NodeLeafAndValue::new::<sha2::Sha256>);
            cache_log.add_read(key.clone(), value.clone());

            let next_value = create_value(5);
            cache_log.add_write(key.clone(), next_value.clone());

            let writes = cache_log.take_writes();
            assert_eq!(writes.len(), 1);
            assert_eq!((key.clone(), next_value), writes[0]);
        }

        // Test that write overrides another write
        {
            let mut cache_log = CacheLog::default();
            let value = create_value(4);
            cache_log.add_write(key.clone(), value.clone());

            let next_value = create_value(5);
            cache_log.add_write(key.clone(), next_value.clone());

            let writes = cache_log.take_writes();
            assert_eq!(writes.len(), 1);
            assert_eq!((key, next_value), writes[0]);
        }
    }
}
