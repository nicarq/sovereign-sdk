use std::convert::Infallible;
use std::marker::PhantomData;

use sov_state::{
    CompileTimeNamespace, IsValueCached, SlotKey, SlotValue, StateCodec, StateItemCodec,
    StateItemDecoder,
};
#[cfg(feature = "native")]
use sov_state::{ProvableCompileTimeNamespace, Storage, StorageProof};

use crate::state::accessors::seal::CachedAccessor;
#[cfg(feature = "native")]
use crate::{ProvenStateAccessor, Spec, StateReaderAndWriter, WorkingSet};
use crate::{StateReader, StateWriter};

pub struct UnmeteredStateWrapper<'a, T: CachedAccessor<N>, N: CompileTimeNamespace> {
    pub(crate) inner: &'a mut T,
    pub(crate) phantom: PhantomData<N>,
}

impl<'a, T: CachedAccessor<N>, N: CompileTimeNamespace> CachedAccessor<N>
    for UnmeteredStateWrapper<'a, T, N>
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

impl<'a, Inner: CachedAccessor<N>, N: CompileTimeNamespace> StateReader<N>
    for UnmeteredStateWrapper<'a, Inner, N>
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

impl<'a, Inner: CachedAccessor<N>, N: CompileTimeNamespace> StateWriter<N>
    for UnmeteredStateWrapper<'a, Inner, N>
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

#[cfg(feature = "native")]
impl<'a, N: ProvableCompileTimeNamespace, S: Spec> ProvenStateAccessor<N>
    for UnmeteredStateWrapper<'a, WorkingSet<S>, N>
where
    WorkingSet<S>: StateReaderAndWriter<N>,
{
    type Proof = <S::Storage as Storage>::Proof;

    fn get_with_proof(&mut self, key: SlotKey) -> StorageProof<Self::Proof> {
        self.inner.get_with_proof(key)
    }
}
