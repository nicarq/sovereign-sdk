use std::convert::Infallible;

use sov_state::{
    CompileTimeNamespace, IsValueCached, SlotKey, SlotValue, StateCodec, StateItemCodec,
    StateItemDecoder,
};

use crate::state::accessors::seal::CachedAccessor;
use crate::{StateReader, StateWriter};

/// A wrapper around an accessor that does not charge gas for state accesses.
/// This is used in the testing framework to wrap the [`crate::WorkingSet`] and avoid charging gas in the `post_dispatch_hook` checks for tests.
/// It is also currently used in the `EVM` module to avoid double-charging gas for state accesses.
pub struct UnmeteredStateWrapper<'a, T> {
    pub(crate) inner: &'a mut T,
}

impl<'a, T: CachedAccessor<N>, N: CompileTimeNamespace> CachedAccessor<N>
    for UnmeteredStateWrapper<'a, T>
{
    fn get_cached(&mut self, key: &SlotKey) -> (Option<SlotValue>, IsValueCached) {
        self.inner.get_cached(key)
    }

    fn set_cached(&mut self, key: &SlotKey, value: SlotValue) -> IsValueCached {
        self.inner.set_cached(key, value)
    }

    fn delete_cached(&mut self, key: &SlotKey) -> IsValueCached {
        self.inner.delete_cached(key)
    }
}

impl<'a, Inner> UnmeteredStateWrapper<'a, Inner> {
    /// Returns a reference to the inner state accessor.
    pub fn inner(&self) -> &Inner {
        self.inner
    }
}

impl<'a, Inner, N: CompileTimeNamespace> StateReader<N> for UnmeteredStateWrapper<'a, Inner>
where
    Inner: StateReader<N>,
{
    type Error = Infallible;

    fn get(&mut self, key: &SlotKey) -> Result<Option<SlotValue>, Self::Error> {
        Ok(self.inner.get_cached(key).0)
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
        let storage_value = <Self as StateReader<N>>::get(self, storage_key)?;

        Ok(storage_value
            .map(|storage_value| codec.value_codec().decode_unwrap(storage_value.value())))
    }
}

impl<'a, Inner, N: CompileTimeNamespace> StateWriter<N> for UnmeteredStateWrapper<'a, Inner>
where
    Inner: StateWriter<N>,
{
    type Error = Infallible;

    fn set(&mut self, key: &SlotKey, value: SlotValue) -> Result<(), Self::Error> {
        <Self as CachedAccessor<N>>::set_cached(self, key, value);
        Ok(())
    }

    fn delete(&mut self, key: &SlotKey) -> Result<(), Self::Error> {
        <Self as CachedAccessor<N>>::delete_cached(self, key);
        Ok(())
    }
}
