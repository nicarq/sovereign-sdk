use sov_rollup_interface::common::VisibleSlotNumber;
use sov_state::{IsValueCached, Namespace, SlotKey, SlotValue, StateAccesses, Storage};
use tracing::trace;

use super::internals::{AccessoryDelta, Delta};
use super::{BootstrapWorkingSet, UniversalStateAccessor};
use crate::capabilities::{Kernel, RollupHeight};
use crate::{Spec, VersionReader};

/// This structure is responsible for storing the `read-write` set.
///
/// A [`StateCheckpoint`] can be obtained from a [`crate::WorkingSet`] in two ways:
///  1. With [`crate::TxScratchpad::commit`].
///  2. With [`crate::WorkingSet::revert`].
pub struct StateCheckpoint<S: Spec> {
    pub(super) delta: Delta<S::Storage>,
    /// The rollup height visible to user-space modules
    pub(super) visible_slot_num: VisibleSlotNumber,
    pub(super) rollup_height: RollupHeight,
}

#[derive(Debug)]
/// The list of changes from the state checkpoint
pub struct ChangeSet {
    #[allow(missing_docs)]
    pub changes: Vec<((SlotKey, sov_state::Namespace), Option<SlotValue>)>,
}

impl ChangeSet {
    /// Create a new `ChangeSet` from a vector of changes.
    pub fn new(changes: Vec<((SlotKey, sov_state::Namespace), Option<SlotValue>)>) -> Self {
        Self { changes }
    }
}

impl<S: Spec> StateCheckpoint<S> {
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
            rollup_height: self.rollup_height,
        }
    }

    /// Creates a new [`StateCheckpoint`] instance without any changes, backed
    /// by the given [`Storage`].
    pub fn new<K: Kernel<S>>(inner: S::Storage, kernel: &K) -> Self {
        Self::with_witness(inner, Default::default(), kernel)
    }

    /// Creates a new [`StateCheckpoint`] instance without any changes, backed
    /// by the given [`Storage`] and witness.
    pub fn with_witness<K: Kernel<S>>(
        inner: S::Storage,
        witness: <S::Storage as Storage>::Witness,
        kernel: &K,
    ) -> Self {
        let mut delta = Delta::with_witness(inner.clone(), witness, None);
        let mut bootstrap_state = BootstrapWorkingSet { inner: &mut delta };

        let visible_slot_num = kernel.next_visible_slot_number(&mut bootstrap_state);
        trace!(%visible_slot_num, "Initializing a `StateCheckpoint`");
        let rollup_height = kernel.rollup_height(&mut bootstrap_state);
        Self {
            delta,
            visible_slot_num,
            rollup_height,
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
        let (state_accesses, accesory_delta, witness, _storage) = self.delta.freeze();
        (state_accesses, accesory_delta, witness)
    }

    /// Extracts ordered reads, writes, and witness from this [`StateCheckpoint`] and uses
    /// them to compute the `StateUpdate` created by this StateCheckpoint.
    #[allow(clippy::type_complexity)]
    pub fn materialize_update(
        self,
    ) -> (
        <S::Storage as Storage>::Root,
        <S::Storage as Storage>::StateUpdate,
        AccessoryDelta<S::Storage>,
        <S::Storage as Storage>::Witness,
        S::Storage,
    ) {
        let (cache_log, accessory_delta, witness, storage) = self.delta.freeze();

        let (root, update) = storage
            .compute_state_update(cache_log, &witness)
            .expect("state update computation must succeed");
        (root, update, accessory_delta, witness, storage)
    }

    /// Updates the true [`SlotNumber`] and the [`VisibleSlotNumber`].
    ///
    /// This method is used in tests.
    #[cfg(test)]
    pub fn update_version(&mut self, visible_slot_number: u64) {
        self.visible_slot_num = VisibleSlotNumber::new_dangerous(visible_slot_number);
    }

    /// Returns the list of all changes contained in the state checkpoint.
    pub fn changes(&self) -> ChangeSet {
        self.delta.changes()
    }

    /// Directly apply a set of changes to the state checkpoint. This method should generally *not* be used
    /// during normal execution, since changes should happen through `StateValue` types which
    /// use the UniversalStateAccessor API. It is primarily intended for use in the sequencer, which has to manage
    /// its own state.
    // TODO: Remove this method if we stop using `StateCheckpoint` in the sequencer
    #[cfg(feature = "native")]
    pub fn apply_tx_changes(&mut self, changeset: super::TxChangeSet) {
        self.apply_changes(changeset.0);
    }

    /// Directly apply a set of changes to the state checkpoint. This method should generally *not* be used
    /// during normal execution, since changes should happen through `StateValue` types which
    /// use the UniversalStateAccessor API. It is primarily intended for use in the sequencer, which has to manage
    /// its own state.
    // TODO: Remove this method if we stop using `StateCheckpoint` in the sequencer
    #[cfg(feature = "native")]
    pub fn apply_changes(&mut self, changeset: ChangeSet) {
        for ((key, namespace), value) in changeset.changes {
            if let Some(value) = value {
                self.set(namespace, &key, value);
            } else {
                self.delete(namespace, &key);
            }
        }
    }
}

impl<S: Spec> VersionReader for StateCheckpoint<S> {
    fn visible_slot_number_to_access(&self) -> VisibleSlotNumber {
        self.visible_slot_num
    }

    fn rollup_height_to_access(&self) -> RollupHeight {
        self.rollup_height
    }
}

impl<S: Spec> UniversalStateAccessor for StateCheckpoint<S> {
    fn get(&mut self, namespace: Namespace, key: &SlotKey) -> (Option<SlotValue>, IsValueCached) {
        self.delta.get(namespace, key)
    }

    fn set(&mut self, namespace: Namespace, key: &SlotKey, value: SlotValue) -> IsValueCached {
        self.delta.set(namespace, key, value)
    }

    fn delete(&mut self, namespace: Namespace, key: &SlotKey) -> IsValueCached {
        self.delta.delete(namespace, key)
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
