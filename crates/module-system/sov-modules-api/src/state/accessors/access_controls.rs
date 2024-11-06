//! This file defines all the possible ways to access the state of the rollup for the
//! accessors defined in this module.

use std::convert::Infallible;

#[cfg(feature = "native")]
use sov_state::Accessory;
use sov_state::{
    Kernel, SlotKey, SlotValue, StateCodec, StateItemCodec, StateItemDecoder, Storage, User,
};

use super::genesis::GenesisStateAccessor;
#[cfg(feature = "native")]
use super::internals::Delta;
use super::seal::CachedAccessor;
use super::StateProvider;
use crate::state::traits::{AccessoryStateWriter, ProvableStateReader, ProvableStateWriter};
#[cfg(feature = "native")]
use crate::AccessoryStateCheckpoint;
use crate::{
    AccessoryDelta, AccessoryStateReader, PreExecWorkingSet, Spec, StateCheckpoint, StateReader,
    StateWriter, TxScratchpad, WorkingSet,
};

macro_rules! inner_impl_unmetered_state_reader {
    ($namespace:ty) => {
        type Error = Infallible;

        fn get(&mut self, key: &SlotKey) -> Result<Option<SlotValue>, Self::Error> {
            Ok(<Self as CachedAccessor<$namespace>>::get_cached(self, key).0)
        }

        fn get_decoded<V, Codec>(
            &mut self,
            storage_key: &SlotKey,
            codec: &Codec,
        ) -> Result<Option<V>, Self::Error>
        where
            Codec: StateCodec,
            Codec::ValueCodec: StateItemCodec<V>,
        {
            let storage_value = <Self as StateReader<$namespace>>::get(self, storage_key)?;

            Ok(storage_value
                .map(|storage_value| codec.value_codec().decode_unwrap(storage_value.value())))
        }
    };
}

macro_rules! inner_impl_unmetered_state_writer {
    ($namespace:ty) => {
        type Error = Infallible;

        fn set(&mut self, key: &SlotKey, value: SlotValue) -> Result<(), Self::Error> {
            <Self as CachedAccessor<$namespace>>::set_cached(self, key, value);
            Ok(())
        }

        fn delete(&mut self, key: &SlotKey) -> Result<(), Self::Error> {
            <Self as CachedAccessor<$namespace>>::delete_cached(self, key);
            Ok(())
        }
    };
}

#[cfg(feature = "native")]
mod http_api {
    use std::convert::Infallible;

    use sov_state::{
        IsValueCached, Kernel, SlotKey, SlotValue, StateCodec, StateItemCodec, StateItemDecoder,
        User,
    };

    use super::CachedAccessor;
    use crate::gas::GasMeter;
    use crate::module::GasSpec;
    use crate::state::accessors::http_api::ApiStateAccessor;
    use crate::state::traits::decode_gas_cost;
    use crate::{AccessoryStateReader, AccessoryStateWriter, Spec, StateReader, StateWriter};

    macro_rules! inner_impl_http_api_state_reader {
        ($namespace:ty) => {
        type Error = Infallible;

        fn get(&mut self, key: &SlotKey) -> Result<Option<SlotValue>, Infallible> {
            self.charge_gas(&S::gas_to_charge_for_access()).expect("We should never fail to charge gas for read operation of api accessors. This is a bug!");

            let (val, is_value_cached) = CachedAccessor::<$namespace>::get_cached(self, key);

            if is_value_cached == IsValueCached::Yes {
                self.refund_gas(&S::gas_to_refund_for_hot_access()).expect("Failed to refund gas for read operation. This is a bug. The gas refund constant should always be lower than the gas to charge.");
            }

            Ok(val)
        }

        fn get_decoded<V, Codec>(
            &mut self,
            storage_key: &SlotKey,
            codec: &Codec,
        ) -> Result<Option<V>, Infallible>
        where
            Codec: StateCodec,
            Codec::ValueCodec: StateItemCodec<V>,
        {
            let storage_value = <Self as StateReader<$namespace>>::get(self, storage_key)?;

            if let Some(storage_value) = &storage_value {
                self.charge_gas(&decode_gas_cost::<S>(storage_value)).expect("We should never fail to charge gas for read operation of api accessors. This is a bug!")
            }

            Ok(storage_value
                .map(|storage_value| codec.value_codec().decode_unwrap(storage_value.value())))
        }
        };
    }

    macro_rules! inner_impl_http_api_state_writer {
        ($namespace:ty) => {
                type Error = Infallible;

                fn set(&mut self, key: &SlotKey, value: SlotValue) -> Result<(), Infallible> {
                    self.charge_gas(&S::gas_to_charge_for_write())
                        .expect("We should never fail to charge gas for write operation of api accessors. This is a bug!");
                    let is_value_cached = CachedAccessor::<$namespace>::set_cached(self, key, value);

                    if is_value_cached == IsValueCached::Yes {
                        self.refund_gas(&S::gas_to_refund_for_hot_write()).expect("Failed to refund gas for write operation. This is a bug. The gas refund constant should always be lower than the gas to charge.");
                    }

                    Ok(())
                }

                fn delete(&mut self, key: &SlotKey) -> Result<(), Infallible> {
                    self.charge_gas(&S::gas_to_charge_for_delete())
                    .expect("We should never fail to charge gas for write operation of api accessors. This is a bug!");

                    let is_value_cached = CachedAccessor::<$namespace>::delete_cached(self, key);

                    if is_value_cached == IsValueCached::Yes {
                        self.refund_gas(&S::gas_to_refund_for_hot_delete()).expect("Failed to refund gas for delete operation. This is a bug. The gas refund constant should always be lower than the gas to charge.");
                    }

                    Ok(())
                }
            }
    }

    impl<S: Spec> AccessoryStateReader for ApiStateAccessor<S> {}
    impl<S: Spec> AccessoryStateWriter for ApiStateAccessor<S> {}

    impl<S: Spec> StateReader<User> for ApiStateAccessor<S> {
        inner_impl_http_api_state_reader!(User);
    }
    impl<S: Spec> StateWriter<User> for ApiStateAccessor<S> {
        inner_impl_http_api_state_writer!(User);
    }

    impl<S: Spec> StateReader<Kernel> for ApiStateAccessor<S> {
        inner_impl_http_api_state_reader!(Kernel);
    }
    impl<S: Spec> StateWriter<Kernel> for ApiStateAccessor<S> {
        inner_impl_http_api_state_writer!(Kernel);
    }
}

impl<S: Storage> AccessoryStateReader for AccessoryDelta<S> {}
impl<S: Storage> AccessoryStateWriter for AccessoryDelta<S> {}

impl<'a, S: Spec> StateReader<User> for GenesisStateAccessor<'a, S> {
    inner_impl_unmetered_state_reader!(User);
}
impl<'a, S: Spec> StateWriter<User> for GenesisStateAccessor<'a, S> {
    inner_impl_unmetered_state_writer!(User);
}
impl<'a, S: Spec> StateReader<Kernel> for GenesisStateAccessor<'a, S> {
    inner_impl_unmetered_state_reader!(Kernel);
}
impl<'a, S: Spec> StateWriter<Kernel> for GenesisStateAccessor<'a, S> {
    inner_impl_unmetered_state_writer!(Kernel);
}
impl<'a, S: Spec> AccessoryStateWriter for GenesisStateAccessor<'a, S> {}

impl<S: Storage> StateReader<User> for StateCheckpoint<S> {
    inner_impl_unmetered_state_reader!(User);
}
impl<S: Storage> StateWriter<User> for StateCheckpoint<S> {
    inner_impl_unmetered_state_writer!(User);
}

impl<S: Storage> StateReader<Kernel> for StateCheckpoint<S> {
    inner_impl_unmetered_state_reader!(Kernel);
}
impl<S: Storage> StateWriter<Kernel> for StateCheckpoint<S> {
    inner_impl_unmetered_state_writer!(Kernel);
}

impl<S: Storage> AccessoryStateWriter for StateCheckpoint<S> {}

impl<S, I> StateReader<User> for TxScratchpad<S, I>
where
    S: Spec,
    I: StateProvider<S>,
{
    inner_impl_unmetered_state_reader!(User);
}
impl<S, I> StateReader<Kernel> for TxScratchpad<S, I>
where
    S: Spec,
    I: StateProvider<S>,
{
    inner_impl_unmetered_state_reader!(Kernel);
}

impl<S: Spec, I: StateProvider<S>> StateWriter<User> for TxScratchpad<S, I> {
    inner_impl_unmetered_state_writer!(User);
}

impl<S: Spec, I: StateProvider<S>> ProvableStateReader<User> for PreExecWorkingSet<S, I> {
    type Spec = S;
}
/// TODO: the [`PreExecWorkingSet`] should not be able to read the kernel state. Make sure
/// to find a way to enforce that.
impl<S: Spec, I: StateProvider<S>> ProvableStateReader<Kernel> for PreExecWorkingSet<S, I> {
    type Spec = S;
}
impl<S: Spec, I: StateProvider<S>> ProvableStateWriter<User> for PreExecWorkingSet<S, I> {
    type Spec = S;
}

impl<S: Spec, I: StateProvider<S>> ProvableStateReader<User> for WorkingSet<S, I> {
    type Spec = S;
}
/// TODO: the [`WorkingSet`] should not be able to read the kernel state. Make sure
/// to find a way to enforce that.
impl<S: Spec, I: StateProvider<S>> ProvableStateReader<Kernel> for WorkingSet<S, I> {
    type Spec = S;
}
impl<S: Spec, I: StateProvider<S>> ProvableStateWriter<User> for WorkingSet<S, I> {
    type Spec = S;
}

impl<S: Spec, I: StateProvider<S>> AccessoryStateWriter for WorkingSet<S, I> {}

#[cfg(feature = "test-utils")]
impl<S: Spec, I: StateProvider<S>> AccessoryStateReader for WorkingSet<S, I> {}

#[cfg(feature = "native")]
impl<'a, S: Storage> StateReader<Accessory> for AccessoryStateCheckpoint<'a, S> {
    type Error = Infallible;
    fn get(&mut self, key: &SlotKey) -> Result<Option<SlotValue>, Self::Error> {
        Ok(<Delta<S> as CachedAccessor<Accessory>>::get_cached(&mut self.checkpoint.delta, key).0)
    }

    /// Get a decoded value from the storage.
    fn get_decoded<V, Codec>(
        &mut self,
        storage_key: &SlotKey,
        codec: &Codec,
    ) -> Result<Option<V>, Self::Error>
    where
        Codec: StateCodec,
        Codec::ValueCodec: StateItemCodec<V>,
    {
        let storage_value = <Self as StateReader<Accessory>>::get(self, storage_key)?;

        Ok(storage_value
            .map(|storage_value| codec.value_codec().decode_unwrap(storage_value.value())))
    }
}

#[cfg(feature = "native")]
impl<'a, S: Storage> AccessoryStateWriter for AccessoryStateCheckpoint<'a, S> {}

pub mod kernel_state {
    use std::convert::Infallible;

    use sov_state::{
        Kernel, SlotKey, SlotValue, StateCodec, StateItemCodec, StateItemDecoder, Storage, User,
    };

    use crate::state::accessors::seal::CachedAccessor;
    use crate::state::accessors::BootstrapWorkingSet;
    use crate::{KernelStateAccessor, StateReader, StateWriter};

    impl<'a, S: Storage> StateReader<Kernel> for BootstrapWorkingSet<'a, S> {
        inner_impl_unmetered_state_reader!(Kernel);
    }
    impl<'a, S: Storage> StateReader<User> for BootstrapWorkingSet<'a, S> {
        inner_impl_unmetered_state_reader!(User);
    }

    impl<'a, S: Storage> StateWriter<Kernel> for BootstrapWorkingSet<'a, S> {
        inner_impl_unmetered_state_writer!(Kernel);
    }
    impl<'a, S: Storage> StateWriter<User> for BootstrapWorkingSet<'a, S> {
        inner_impl_unmetered_state_writer!(User);
    }

    impl<'a, S: Storage> StateReader<Kernel> for KernelStateAccessor<'a, S> {
        inner_impl_unmetered_state_reader!(Kernel);
    }
    impl<'a, S: Storage> StateReader<User> for KernelStateAccessor<'a, S> {
        inner_impl_unmetered_state_reader!(User);
    }

    impl<'a, S: Storage> StateWriter<Kernel> for KernelStateAccessor<'a, S> {
        inner_impl_unmetered_state_writer!(Kernel);
    }
    impl<'a, S: Storage> StateWriter<User> for KernelStateAccessor<'a, S> {
        inner_impl_unmetered_state_writer!(User);
    }
}
