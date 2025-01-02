use sov_rollup_interface::common::{SlotNumber, VisibleSlotNumber};
use sov_state::{IsValueCached, Namespace, SlotKey, SlotValue, StateAccesses, Storage};

use super::internals::{AccessoryDelta, Delta};
use super::{BootstrapWorkingSet, UniversalStateAccessor};
use crate::capabilities::Kernel;
use crate::{Spec, VersionReader};

/// This structure is responsible for storing the `read-write` set.
///
/// A [`StateCheckpoint`] can be obtained from a [`crate::WorkingSet`] in two ways:
///  1. With [`crate::TxScratchpad::commit`].
///  2. With [`crate::WorkingSet::revert`].
pub struct StateCheckpoint<S: Storage> {
    pub(super) delta: Delta<S>,
    /// The rollup height visible to user-space modules
    pub(super) visible_slot_num: VisibleSlotNumber,
}

impl<S: Storage> StateCheckpoint<S> {
    /// Deep copy the state checkpoint (including its caches), ignoring
    /// the witness.
    ///
    /// Since this method leaves the witness of the new
    /// checkpoint in a state that is inconsistent with its caches,
    /// it should only be used in situations where the witness is not needed,
    /// such as in the API accessors.
    pub fn clone_with_empty_witness(&self) -> Self {
        Self {
            delta: self.delta.clone_with_empty_witness(),
            visible_slot_num: self.visible_slot_num,
        }
    }

    /// Creates a new [`StateCheckpoint`] instance without any changes, backed
    /// by the given [`Storage`].
    pub fn new<Sp: Spec<Storage = S>, K: Kernel<Sp>>(inner: S, kernel: &K) -> Self {
        Self::with_witness(inner, Default::default(), kernel)
    }

    /// Creates a new [`StateCheckpoint`] instance without any changes, backed
    /// by the given [`Storage`] and witness.
    pub fn with_witness<Sp: Spec<Storage = S>, K: Kernel<Sp>>(
        inner: S,
        witness: S::Witness,
        kernel: &K,
    ) -> Self {
        let mut delta = Delta::with_witness(inner.clone(), witness, None);
        let mut bootstrap_state = BootstrapWorkingSet { inner: &mut delta };

        let visible_slot_num = kernel.next_visible_rollup_height(&mut bootstrap_state);

        Self {
            delta,
            visible_slot_num,
        }
    }

    /// Extracts ordered reads, writes, and witness from this [`StateCheckpoint`].
    ///
    /// You can then use these to call [`Storage::validate_and_materialize`] or some
    /// of the other related [`Storage`] methods. Note that this data is moved
    /// **out** of the [`StateCheckpoint`] i.e. it can't be extracted twice.
    pub fn freeze(self) -> (StateAccesses, AccessoryDelta<S>, S::Witness) {
        let (state_accesses, accesory_delta, witness, _storage) = self.delta.freeze();
        (state_accesses, accesory_delta, witness)
    }

    /// Extracts ordered reads, writes, and witness from this [`StateCheckpoint`] and uses
    /// them to compute the `StateUpdate` created by this StateCheckpoint.
    pub fn materialize_update(
        self,
    ) -> (
        <S as Storage>::Root,
        <S as Storage>::StateUpdate,
        AccessoryDelta<S>,
        S::Witness,
        S,
    ) {
        let (cache_log, accessory_delta, witness, storage) = self.delta.freeze();

        let (root, update) = storage
            .compute_state_update(cache_log, &witness)
            .expect("state update computation must succeed");
        (root, update, accessory_delta, witness, storage)
    }

    /// Updates the true rollup height and the visible rollup height.
    /// This method is used in tests.
    #[cfg(test)]
    pub fn update_version(&mut self, visible_slot_num: u64) {
        use sov_rollup_interface::common::IntoSlotNumber;

        self.visible_slot_num = visible_slot_num.to_visible_slot_number();
    }

    /// Directly apply a set of changes to the state checkpoint. This method should generally *not* be used
    /// during normal execution, since changes should happen through `StateValue` types which
    /// use the UniversalStateAccessor API. It is primarily intended for use in the sequencer, which has to manage
    /// its own state.
    // TODO: Remove this method when we stop using `StateCheckpoint` in the sequencer
    #[cfg(feature = "native")]
    pub fn apply_changes(&mut self, changeset: super::TxChangeSet) {
        for ((key, namespace), value) in changeset.changes {
            if let Some(value) = value {
                self.set(namespace, &key, value);
            } else {
                self.delete(namespace, &key);
            }
        }
    }
}

impl<S: Storage> VersionReader for StateCheckpoint<S> {
    fn rollup_height_to_access(&self) -> SlotNumber {
        self.visible_slot_num.as_true()
    }
}

impl<S: Storage> UniversalStateAccessor for StateCheckpoint<S> {
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
    use sov_state::{Accessory, IsValueCached, SlotKey, SlotValue, Storage};

    use crate::state::accessors::seal::CachedAccessor;
    use crate::StateCheckpoint;

    impl<S: Storage> StateCheckpoint<S> {
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
    pub struct AccessoryStateCheckpoint<'a, S: Storage> {
        pub(in crate::state) checkpoint: &'a mut StateCheckpoint<S>,
    }

    impl<'a, S: Storage> CachedAccessor<Accessory> for AccessoryStateCheckpoint<'a, S> {
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
