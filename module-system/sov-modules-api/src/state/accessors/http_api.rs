use sov_state::{CompileTimeNamespace, IsValueCached, SlotKey, SlotValue};

use super::internals::Delta;
use super::seal::CachedAccessor;
use crate::Spec;

/// A storage wrapper that can be used to access the state inside http api requests.
/// This is the data structure that should be used inside RPC and REST macros to generate storage accessors.
///
/// ## Usage note
/// This method does not charge for read/write operations. Transaction simulation through the http api will use a
/// different storage accessor that has less permissions that this one. In particular reading operations to the accessory
/// state won't be allowed.
pub struct ApiStateAccessor<S: Spec> {
    pub(super) delta: Delta<S::Storage>,
}

impl<S: Spec> ApiStateAccessor<S> {
    /// Creates a new [`ApiStateAccessor`] instance backed by the given [`Spec::Storage`].
    pub fn new(inner: S::Storage) -> Self {
        Self {
            delta: Delta::new(inner.clone(), None),
        }
    }

    fn storage(&self) -> &S::Storage {
        &self.delta.inner
    }

    /// Creates a new archival rest state checkpoint with the same underlying `Storage` but an empty Delta, without
    /// modifying the original [`ApiStateAccessor`].
    pub fn get_archival_at(&self, version: u64) -> Self {
        let storage = self.storage().clone();

        Self {
            delta: Delta::new(storage.clone(), Some(version)),
        }
    }
}

impl<S: Spec, N: CompileTimeNamespace> CachedAccessor<N> for ApiStateAccessor<S> {
    fn get_cached(&mut self, key: &SlotKey) -> (Option<SlotValue>, IsValueCached) {
        CachedAccessor::<N>::get_cached(&mut self.delta, key)
    }

    fn set_cached(&mut self, key: &SlotKey, value: SlotValue) -> IsValueCached {
        CachedAccessor::<N>::set_cached(&mut self.delta, key, value)
    }

    fn delete_cached(&mut self, key: &SlotKey) -> IsValueCached {
        CachedAccessor::<N>::delete_cached(&mut self.delta, key)
    }
}
