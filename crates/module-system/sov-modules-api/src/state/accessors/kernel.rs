use internals::Delta;
use sov_rollup_interface::common::{SlotNumber, VisibleSlotNumber};
/// Provides specialized working set wrappers for dealing with protected state.
use sov_state::{IsValueCached, SlotKey, SlotValue, Storage};

use self::checkpoints::StateCheckpoint;
use super::*;
use crate::capabilities::{Kernel, RollupHeight};
use crate::state::traits::{KernelWriter, VersionReader};
use crate::Spec;

/// A special wrapper over a `Delta` on the storage that allows access to kernel values to bootstrap the [`StateCheckpoint`].
pub struct BootstrapWorkingSet<'a, S: Storage> {
    /// The inner working set
    pub(super) inner: &'a mut Delta<S>,
}

impl<'a, S: Storage, N: CompileTimeNamespace> CachedAccessor<N> for BootstrapWorkingSet<'a, S> {
    fn get_cached(&mut self, key: &SlotKey) -> (Option<SlotValue>, IsValueCached) {
        <Delta<S> as CachedAccessor<N>>::get_cached(self.inner, key)
    }

    fn set_cached(&mut self, key: &SlotKey, value: SlotValue) -> IsValueCached {
        <Delta<S> as CachedAccessor<N>>::set_cached(self.inner, key, value)
    }

    fn delete_cached(&mut self, key: &SlotKey) -> IsValueCached {
        <Delta<S> as CachedAccessor<N>>::delete_cached(self.inner, key)
    }
}

/// A special wrapper over [`StateCheckpoint`] that allows access to kernel values
///
/// ## Note
/// This struct implements [`VersionReader`], and the value returned by
/// [`VersionReader::rollup_height_to_access`] is the last known rollup height.
pub struct KernelStateAccessor<'a, S: Spec> {
    /// The inner working set
    pub checkpoint: &'a mut StateCheckpoint<S>,
    pub(crate) true_slot_num: SlotNumber,
}

impl<'a, S: Spec> VersionReader for KernelStateAccessor<'a, S> {
    fn visible_slot_number_to_access(&self) -> VisibleSlotNumber {
        VisibleSlotNumber::new_dangerous(self.true_slot_num.get())
    }

    fn rollup_height_to_access(&self) -> RollupHeight {
        self.checkpoint.rollup_height
    }
}

impl<'a, S: Spec> KernelWriter for KernelStateAccessor<'a, S> {
    fn true_slot_number(&self) -> SlotNumber {
        self.true_slot_num
    }
}

impl<'a, S: Spec> KernelStateAccessor<'a, S> {
    /// Instantiates a new [`KernelStateAccessor`].
    pub fn from_checkpoint<K: Kernel<S> + ?Sized>(
        kernel: &K,
        checkpoint: &'a mut StateCheckpoint<S>,
    ) -> Self {
        let mut bootstrap = BootstrapWorkingSet {
            inner: &mut checkpoint.delta,
        };

        let true_slot_num = kernel.true_slot_number(&mut bootstrap);

        Self {
            checkpoint,
            true_slot_num,
        }
    }
}

impl<'a, S: Spec> KernelStateAccessor<'a, S> {
    /// Returns the visible rollup height contained in the accessor
    pub fn visible_slot_number(&self) -> VisibleSlotNumber {
        self.checkpoint.visible_slot_num
    }

    /// Updates the true rollup height contained in the accessor
    pub fn update_true_slot_number(&mut self, true_slot_num: SlotNumber) {
        self.true_slot_num = true_slot_num;
    }

    /// Updates the visible rollup height contained in the accessor
    pub fn update_visible_slot_number(&mut self, visible_slot_num: VisibleSlotNumber) {
        self.checkpoint.visible_slot_num = visible_slot_num;
    }

    /// Updates the visible rollup height contained in the accessor
    pub fn update_rollup_height(&mut self, rollup_height: RollupHeight) {
        self.checkpoint.rollup_height = rollup_height;
    }
}

impl<S: Spec> UniversalStateAccessor for KernelStateAccessor<'_, S> {
    fn get(&mut self, namespace: Namespace, key: &SlotKey) -> (Option<SlotValue>, IsValueCached) {
        UniversalStateAccessor::get(self.checkpoint, namespace, key)
    }

    fn set(&mut self, namespace: Namespace, key: &SlotKey, value: SlotValue) -> IsValueCached {
        UniversalStateAccessor::set(self.checkpoint, namespace, key, value)
    }

    fn delete(&mut self, namespace: Namespace, key: &SlotKey) -> IsValueCached {
        UniversalStateAccessor::delete(self.checkpoint, namespace, key)
    }
}
