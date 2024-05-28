//! This file defines all the possible ways to access the state of the rollup for the
//! accessors defined in this module.

use sov_state::{Accessory, SlotKey, SlotValue, Storage, User};

use super::genesis::GenesisStateAccessor;
use super::internals::{Delta, RevertableWriter};
use super::seal::CachedAccessor;
use crate::{
    AccessoryDelta, AccessoryStateCheckpoint, GasMeter, PreExecWorkingSet, Spec, StateCheckpoint,
    StateReader, StateWriter, TxScratchpad, WorkingSet,
};

impl<S: Storage> StateReader<Accessory> for AccessoryDelta<S> {}
impl<S: Storage> StateWriter<Accessory> for AccessoryDelta<S> {}

impl<S: Spec> StateReader<User> for GenesisStateAccessor<S> {}
impl<S: Spec> StateWriter<User> for GenesisStateAccessor<S> {}
impl<S: Spec> StateWriter<Accessory> for GenesisStateAccessor<S> {}

impl<S: Spec> StateReader<User> for StateCheckpoint<S> {}
impl<S: Spec> StateWriter<User> for StateCheckpoint<S> {}
impl<S: Spec> StateWriter<Accessory> for StateCheckpoint<S> {}

impl<S: Spec> StateReader<User> for TxScratchpad<S> {}
impl<S: Spec> StateWriter<User> for TxScratchpad<S> {}

impl<S: Spec, PreExecChecksMeter: GasMeter<S::Gas>> StateReader<User>
    for PreExecWorkingSet<S, PreExecChecksMeter>
{
}
impl<S: Spec, PreExecChecksMeter: GasMeter<S::Gas>> StateWriter<User>
    for PreExecWorkingSet<S, PreExecChecksMeter>
{
}

impl<S: Spec> StateReader<User> for WorkingSet<S> {}
impl<S: Spec> StateWriter<User> for WorkingSet<S> {}
impl<S: Spec> StateReader<Accessory> for WorkingSet<S> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        if !cfg!(feature = "native") {
            // Note: We might want to have a special case for that
            panic!("Trying to access a native-protected value {key:?}, from the accessory state, outside of native mode");
        } else {
            <RevertableWriter<TxScratchpad<S>> as CachedAccessor<Accessory>>::get_cached(
                &mut self.delta,
                key,
            )
            .0
        }
    }
}
impl<S: Spec> StateWriter<Accessory> for WorkingSet<S> {}

impl<'a, S: Spec> StateReader<Accessory> for AccessoryStateCheckpoint<'a, S> {
    fn get(&mut self, key: &SlotKey) -> Option<SlotValue> {
        if !cfg!(feature = "native") {
            // Note: We might want to have a special case for that
            panic!("Trying to access a native-protected value {key:?}, from the accessory state, outside of native mode");
        } else {
            <Delta<S::Storage> as CachedAccessor<Accessory>>::get_cached(
                &mut self.checkpoint.delta,
                key,
            )
            .0
        }
    }
}
impl<'a, S: Spec> StateWriter<Accessory> for AccessoryStateCheckpoint<'a, S> {}

pub mod kernel_state {
    use sov_state::{namespaces, User};

    use crate::{
        BootstrapWorkingSet, KernelWorkingSet, Spec, StateCheckpoint, StateReader, StateWriter,
        VersionedStateReadWriter, WorkingSet,
    };

    impl<'a, S: Spec> StateReader<namespaces::Kernel>
        for VersionedStateReadWriter<'a, StateCheckpoint<S>>
    {
    }

    impl<'a, S: Spec> StateWriter<namespaces::Kernel>
        for VersionedStateReadWriter<'a, StateCheckpoint<S>>
    {
    }

    impl<'a, S: Spec> StateReader<namespaces::Kernel> for VersionedStateReadWriter<'a, WorkingSet<S>> {}

    impl<'a, S: Spec> StateWriter<namespaces::Kernel> for VersionedStateReadWriter<'a, WorkingSet<S>> {}

    impl<'a, S: Spec> StateReader<User> for BootstrapWorkingSet<'a, S> {}

    impl<'a, S: Spec> StateWriter<User> for BootstrapWorkingSet<'a, S> {}

    impl<'a, S: Spec> StateReader<namespaces::Kernel> for BootstrapWorkingSet<'a, S> {}

    impl<'a, S: Spec> StateWriter<namespaces::Kernel> for BootstrapWorkingSet<'a, S> {}

    impl<'a, S: Spec> StateReader<User> for KernelWorkingSet<'a, S> {}

    impl<'a, S: Spec> StateWriter<User> for KernelWorkingSet<'a, S> {}
    impl<'a, S: Spec> StateReader<namespaces::Kernel> for KernelWorkingSet<'a, S> {}

    impl<'a, S: Spec> StateWriter<namespaces::Kernel> for KernelWorkingSet<'a, S> {}
}
