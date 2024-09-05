use internals::Delta;
/// Provides specialized working set wrappers for dealing with protected state.
use sov_state::{IsValueCached, SlotKey, SlotValue, Storage};

use self::checkpoints::StateCheckpoint;
use super::*;
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

/// A special wrapper over [`StateCheckpoint`] that allows access to kernel values inside the [`crate::runtime::capabilities::KernelSlotHooks`]
///
/// ## Note
/// This struct implements [`VersionReader`], and the value returned by [`VersionReader::rollup_height_to_access`] is the true slot number.
pub struct KernelStateAccessor<'a, S: Spec>(
    /// The inner working set
    pub &'a mut StateCheckpoint<S>,
);

impl<'a, S: Spec> VersionReader for KernelStateAccessor<'a, S> {
    fn rollup_height_to_access(&self) -> u64 {
        self.0.true_slot_num
    }
}

impl<'a, S: Spec> KernelWriter for KernelStateAccessor<'a, S> {
    fn true_slot_number(&self) -> u64 {
        self.0.true_slot_num
    }
}

impl<'a, S: Spec> From<&'a mut StateCheckpoint<S>> for KernelStateAccessor<'a, S> {
    fn from(value: &'a mut StateCheckpoint<S>) -> Self {
        Self(value)
    }
}

impl<'a, S: Spec> KernelStateAccessor<'a, S> {
    /// Returns the virtual slot number contained in the accessor
    pub fn virtual_slot_number(&self) -> u64 {
        self.0.virtual_slot_num
    }

    /// Updates the true slot number contained in the accessor
    pub fn update_true_slot_number(&mut self, true_slot_num: u64) {
        self.0.true_slot_num = true_slot_num;
    }

    /// Updates the virtual slot number contained in the accessor
    pub fn update_virtual_slot_number(&mut self, virtual_slot_num: u64) {
        self.0.virtual_slot_num = virtual_slot_num;
    }
}

impl<S: Spec> UniversalStateAccessor for KernelStateAccessor<'_, S> {
    fn get(&mut self, namespace: Namespace, key: &SlotKey) -> (Option<SlotValue>, IsValueCached) {
        UniversalStateAccessor::get(self.0, namespace, key)
    }

    fn set(&mut self, namespace: Namespace, key: &SlotKey, value: SlotValue) -> IsValueCached {
        UniversalStateAccessor::set(self.0, namespace, key, value)
    }

    fn delete(&mut self, namespace: Namespace, key: &SlotKey) -> IsValueCached {
        UniversalStateAccessor::delete(self.0, namespace, key)
    }
}
