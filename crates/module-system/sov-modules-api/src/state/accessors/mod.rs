//! Defines traits for storage access

use std::marker::PhantomData;

use internals::RevertableWriter;
use sov_state::{CompileTimeNamespace, IsValueCached, Namespace, SlotKey, SlotValue};

mod access_controls;
mod checkpoints;
mod genesis;
mod internals;
#[cfg(any(feature = "test-utils", feature = "evm"))]
mod unmetered_state_wrapper;

#[cfg(any(feature = "test-utils", feature = "evm"))]
pub use unmetered_state_wrapper::UnmeteredStateWrapper;

#[cfg(feature = "native")]
mod http_api;

#[cfg(feature = "native")]
pub use http_api::ApiStateAccessor;

mod scratchpad;

mod kernel;

#[cfg(feature = "native")]
pub use checkpoints::native::AccessoryStateCheckpoint;
pub use checkpoints::StateCheckpoint;
pub use genesis::GenesisStateAccessor;
pub use internals::AccessoryDelta;
pub use kernel::{BootstrapWorkingSet, KernelStateAccessor};
pub use scratchpad::{PreExecWorkingSet, TxChangeSet, TxScratchpad, WorkingSet};

use self::seal::*;
use super::{StateReaderAndWriter, VersionReader};
use crate::Spec;

pub(super) mod seal {
    use sov_state::{CompileTimeNamespace, IsValueCached, Namespace, SlotKey, SlotValue};

    /// A helper trait that is used to derive [`crate::StateReader`]/[`crate::StateWriter`] and the corresponding unmetered versions.
    pub trait CachedAccessor<N: CompileTimeNamespace> {
        fn get_cached(&mut self, key: &SlotKey) -> (Option<SlotValue>, IsValueCached);
        fn set_cached(&mut self, key: &SlotKey, value: SlotValue) -> IsValueCached;
        fn delete_cached(&mut self, key: &SlotKey) -> IsValueCached;
    }

    /// A helper trait allowing a type to access any namespace by their *runtime* enum variant.
    /// Structs that implements this trait also implement [`CachedAccessor`] for any namespace by default.
    /// Useful to represent structs with caches containing different state value namespaces that can be committed to the storage.
    pub trait UniversalStateAccessor {
        fn get(
            &mut self,
            namespace: Namespace,
            key: &SlotKey,
        ) -> (Option<SlotValue>, IsValueCached);
        fn set(&mut self, namespace: Namespace, key: &SlotKey, value: SlotValue) -> IsValueCached;
        fn delete(&mut self, namespace: Namespace, key: &SlotKey) -> IsValueCached;
    }
}

impl<N: CompileTimeNamespace, T: UniversalStateAccessor> CachedAccessor<N> for T {
    fn get_cached(&mut self, key: &SlotKey) -> (Option<SlotValue>, IsValueCached) {
        <Self as UniversalStateAccessor>::get(self, N::NAMESPACE, key)
    }

    fn set_cached(&mut self, key: &SlotKey, value: SlotValue) -> IsValueCached {
        <Self as UniversalStateAccessor>::set(self, N::NAMESPACE, key, value)
    }

    fn delete_cached(&mut self, key: &SlotKey) -> IsValueCached {
        <Self as UniversalStateAccessor>::delete(self, N::NAMESPACE, key)
    }
}

/// A state abstraction that can be used to kickstart transaction execution.
///
/// See [`StateCheckpoint`], which is the canonical implementation of this
/// trait.
///
/// This is a **sealed trait**.
pub trait StateProvider<S: Spec>:
    Sized
    + UniversalStateAccessor
    + StateReaderAndWriter<sov_state::User>
    + StateReaderAndWriter<sov_state::Kernel>
    + VersionReader
{
    /// Transforms this [`StateProvider`] into a [`TxScratchpad`].
    fn to_tx_scratchpad(self) -> TxScratchpad<S, Self>;
}

impl<S: Spec> StateProvider<S> for StateCheckpoint<S> {
    fn to_tx_scratchpad(self) -> TxScratchpad<S, StateCheckpoint<S>> {
        TxScratchpad {
            inner: RevertableWriter::new(self),
            phantom: PhantomData,
        }
    }
}
