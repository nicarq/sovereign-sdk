use sov_state::{IsValueCached, Namespace, SlotKey, SlotValue, StateAccesses, Storage};

use super::internals::{AccessoryDelta, Delta};
use super::UniversalStateAccessor;
use crate::{Context, Spec, VersionedStateReadWriter};

/// This structure is responsible for storing the `read-write` set.
///
/// A [`StateCheckpoint`] can be obtained from a [`crate::WorkingSet`] in two ways:
///  1. With [`crate::WorkingSet::checkpoint`].
///  2. With [`crate::WorkingSet::revert`].
pub struct StateCheckpoint<S: Spec> {
    pub(super) delta: Delta<S::Storage>,
}

impl<S: Spec> StateCheckpoint<S> {
    /// Creates a new [`StateCheckpoint`] instance without any changes, backed
    /// by the given [`Storage`].
    pub fn new(inner: S::Storage) -> Self {
        Self {
            delta: Delta::new(inner.clone(), None),
        }
    }

    /// Returns a handler for the kernel state (priveleged jmt state)
    ///
    /// You can use this method when calling getters and setters on accessory
    /// state containers, like KernelStateMap.
    pub fn versioned_state(&mut self, context: &Context<S>) -> VersionedStateReadWriter<Self> {
        VersionedStateReadWriter {
            state: self,
            slot_num: context.visible_slot_number(),
        }
    }

    /// Creates a new [`StateCheckpoint`] instance without any changes, backed
    /// by the given [`Storage`] and witness.
    pub fn with_witness(inner: S::Storage, witness: <S::Storage as Storage>::Witness) -> Self {
        Self {
            delta: Delta::with_witness(inner.clone(), witness, None),
        }
    }

    /// Extracts ordered reads, writes, and witness from this [`StateCheckpoint`].
    ///
    /// You can then use these to call [`Storage::validate_and_materialize`] or some
    /// of the other related [`Storage`] methods. Note that this data is moved
    /// **out** of the [`StateCheckpoint`] i.e. it can't be extracted twice.
    pub fn freeze(
        self,
    ) -> (
        StateAccesses,
        AccessoryDelta<S::Storage>,
        <S::Storage as Storage>::Witness,
    ) {
        self.delta.freeze()
    }
}

impl<S: Spec> UniversalStateAccessor for StateCheckpoint<S> {
    fn get(&mut self, namespace: Namespace, key: &SlotKey) -> (Option<SlotValue>, IsValueCached) {
        UniversalStateAccessor::get(&mut self.delta, namespace, key)
    }

    fn set(&mut self, namespace: Namespace, key: &SlotKey, value: SlotValue) -> IsValueCached {
        UniversalStateAccessor::set(&mut self.delta, namespace, key, value)
    }

    fn delete(&mut self, namespace: Namespace, key: &SlotKey) -> IsValueCached {
        UniversalStateAccessor::delete(&mut self.delta, namespace, key)
    }
}

#[cfg(feature = "native")]
pub mod native {
    use sov_state::{Accessory, IsValueCached, SlotKey, SlotValue};

    use crate::state::accessors::seal::CachedAccessor;
    use crate::{Spec, StateCheckpoint};

    impl<S: Spec> StateCheckpoint<S> {
        /// Returns a handler for the accessory state (non-JMT state).
        ///
        /// You can use this method when calling getters and setters on accessory
        /// state containers, like AccessoryStateMap.
        pub fn accessory_state(&mut self) -> AccessoryStateCheckpoint<S> {
            AccessoryStateCheckpoint { checkpoint: self }
        }
    }

    /// A wrapper over [`crate::StateCheckpoint`] that only allows access to the accessory
    /// state (non-JMT state).
    pub struct AccessoryStateCheckpoint<'a, S: Spec> {
        pub(in crate::state) checkpoint: &'a mut StateCheckpoint<S>,
    }

    impl<'a, S: Spec> CachedAccessor<Accessory> for AccessoryStateCheckpoint<'a, S> {
        fn get_cached(&mut self, key: &SlotKey) -> (Option<SlotValue>, IsValueCached) {
            <StateCheckpoint<S> as CachedAccessor<Accessory>>::get_cached(self.checkpoint, key)
        }

        fn set_cached(&mut self, key: &SlotKey, value: SlotValue) -> IsValueCached {
            <StateCheckpoint<S> as CachedAccessor<Accessory>>::set_cached(
                self.checkpoint,
                key,
                value,
            )
        }

        fn delete_cached(&mut self, key: &SlotKey) -> IsValueCached {
            <StateCheckpoint<S> as CachedAccessor<Accessory>>::delete_cached(self.checkpoint, key)
        }
    }
}
