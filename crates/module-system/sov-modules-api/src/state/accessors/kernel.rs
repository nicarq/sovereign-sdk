use internals::Delta;
/// Provides specialized working set wrappers for dealing with protected state.
use sov_state::{IsValueCached, SlotKey, SlotValue, Storage};

use self::checkpoints::StateCheckpoint;
use super::*;
use crate::capabilities::Kernel;
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
pub struct KernelStateAccessor<'a, S: Storage> {
    /// The inner working set
    pub checkpoint: &'a mut StateCheckpoint<S>,
    pub(crate) true_slot_num: u64,
}

impl<'a, S: Storage> VersionReader for KernelStateAccessor<'a, S> {
    fn rollup_height_to_access(&self) -> u64 {
        self.true_slot_num
    }
}

impl<'a, S: Storage> KernelWriter for KernelStateAccessor<'a, S> {
    fn true_slot_number(&self) -> u64 {
        self.true_slot_num
    }
}

impl<'a, S: Storage> KernelStateAccessor<'a, S> {
    /// Instantiates a new [`KernelStateAccessor`].
    pub fn from_checkpoint<Sp: Spec<Storage = S>, K: Kernel<Sp>>(
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

impl<'a, S: Storage> KernelStateAccessor<'a, S> {
    /// Returns the virtual slot number contained in the accessor
    pub fn virtual_slot_number(&self) -> u64 {
        self.checkpoint.virtual_slot_num
    }

    /// Updates the true slot number contained in the accessor
    pub fn update_true_slot_number(&mut self, true_slot_num: u64) {
        self.true_slot_num = true_slot_num;
    }

    /// Updates the virtual slot number contained in the accessor
    pub fn update_virtual_slot_number(&mut self, virtual_slot_num: u64) {
        self.checkpoint.virtual_slot_num = virtual_slot_num;
    }
}

impl<S: Storage> UniversalStateAccessor for KernelStateAccessor<'_, S> {
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
