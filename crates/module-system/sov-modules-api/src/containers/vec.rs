use std::iter::FusedIterator;
use std::marker::PhantomData;

use sov_state::codec::BorshCodec;
use sov_state::namespaces::{Accessory, CompileTimeNamespace, Kernel, User};
use sov_state::{EncodeLike, Prefix, StateCodec, StateItemCodec};
use thiserror::Error;
use unwrap_infallible::UnwrapInfallible;

use super::map::NamespacedStateMap;
use super::value::NamespacedStateValue;
use crate::{InfallibleStateReaderAndWriter, StateReader, StateReaderAndWriter, StateWriter};

/// A growable array of values stored as JMT-backed state.
#[derive(
    Debug,
    Clone,
    PartialEq,
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct NamespacedStateVec<N, V, Codec = BorshCodec> {
    _phantom: PhantomData<(N, V)>,
    pub(crate) prefix: Prefix,
    pub(crate) len_value: NamespacedStateValue<N, usize, Codec>,
    pub(crate) elems: NamespacedStateMap<N, usize, V, Codec>,
}

/// An error type for vector getters.
#[derive(Debug, Error)]
pub enum StateVecError<N> {
    /// Operation failed because the index was out of bounds.
    #[error("Index out of bounds for index: {0} with namespace {}", std::any::type_name::<N>())]
    IndexOutOfBounds(usize),
    /// Value not found.
    #[error("Value not found for prefix: {0} and index: {1} with namespace {}", std::any::type_name::<N>())]
    MissingValue(Prefix, usize, PhantomData<N>),
}

type StateVecResult<N, V> = Result<V, StateVecError<N>>;

/// A vector of state values stored in the user namespace.
pub type StateVec<V, Codec = BorshCodec> = NamespacedStateVec<User, V, Codec>;
/// A vector of state values stored in the accessory namespace.
pub type AccessoryStateVec<V, Codec = BorshCodec> = NamespacedStateVec<Accessory, V, Codec>;
/// A vector of state values stored in the kernel namespace.
pub type KernelStateVec<V, Codec = BorshCodec> = NamespacedStateVec<Kernel, V, Codec>;

impl<N, V> NamespacedStateVec<N, V>
where
    BorshCodec: StateItemCodec<V>,
    N: CompileTimeNamespace,
{
    /// Crates a new [`StateVec`] with the given prefix and the default
    /// [`StateItemCodec`] (i.e. [`BorshCodec`]).
    pub fn new(prefix: Prefix) -> Self {
        Self::with_codec(prefix, BorshCodec)
    }
}

impl<N: CompileTimeNamespace, V, Codec: Clone> NamespacedStateVec<N, V, Codec>
where
    Codec: StateCodec,
    Codec::ValueCodec: StateItemCodec<V> + StateItemCodec<usize>,
    Codec::KeyCodec: StateItemCodec<usize>,
{
    /// Creates a new [`StateVec`] with the given prefix and codec.
    pub fn with_codec(prefix: Prefix, codec: Codec) -> Self {
        // Differentiating the prefixes for the length and the elements
        // shouldn't be necessary, but it's best not to rely on implementation
        // details of `StateValue` and `StateMap` as they both have the right to
        // reserve the whole key space for themselves.
        let len_value = NamespacedStateValue::with_codec(prefix.extended(b"l"), codec.clone());
        let elems = NamespacedStateMap::with_codec(prefix.extended(b"e"), codec);
        Self {
            _phantom: PhantomData,
            prefix,
            len_value,
            elems,
        }
    }

    /// Returns the prefix used when this [`StateVec`] was created.
    pub fn prefix(&self) -> &Prefix {
        &self.prefix
    }

    fn set_len<Writer: StateWriter<N>>(
        &self,
        length: usize,
        state: &mut Writer,
    ) -> Result<(), Writer::Error> {
        self.len_value.set(&length, state)
    }

    fn elems(&self) -> &NamespacedStateMap<N, usize, V, Codec> {
        &self.elems
    }

    fn len_value(&self) -> &NamespacedStateValue<N, usize, Codec> {
        &self.len_value
    }

    /// Sets a value in the vector.
    /// If the index is out of bounds, returns an error.
    /// To push a value to the end of the StateVec, use [`NamespacedStateVec::push`].
    pub fn set<Vq, ReaderAndWriter>(
        &self,
        index: usize,
        value: &Vq,
        state: &mut ReaderAndWriter,
    ) -> Result<Result<(), StateVecError<N>>, <ReaderAndWriter as StateWriter<N>>::Error>
    where
        Vq: ?Sized,
        Codec::ValueCodec: EncodeLike<Vq, V>,
        ReaderAndWriter: StateReaderAndWriter<N>,
    {
        let len = self.len(state)?;

        Ok(if index < len {
            self.elems().set(&index, value, state)?;
            Ok(())
        } else {
            Err(StateVecError::IndexOutOfBounds(index))
        })
    }

    /// Returns the value for the given index.
    pub fn get<Reader: StateReader<N>>(
        &self,
        index: usize,
        state: &mut Reader,
    ) -> Result<Option<V>, Reader::Error> {
        self.elems().get(&index, state)
    }

    /// Returns the value for the given index.
    /// If the index is out of bounds, returns an error.
    /// If the value is absent, returns an error.
    pub fn get_or_err<ReaderAndWriter: StateReaderAndWriter<N>>(
        &self,
        index: usize,
        state: &mut ReaderAndWriter,
    ) -> Result<StateVecResult<N, V>, <ReaderAndWriter as StateWriter<N>>::Error> {
        let len = self.len(state)?;

        Ok(if index < len {
            self.elems().get(&index, state)?.ok_or_else(|| {
                StateVecError::MissingValue(self.prefix().clone(), index, PhantomData)
            })
        } else {
            Err(StateVecError::IndexOutOfBounds(index))
        })
    }

    /// Returns the length of the vector.
    pub fn len<Reader: StateReader<N>>(&self, state: &mut Reader) -> Result<usize, Reader::Error> {
        Ok(self.len_value().get(state)?.unwrap_or_default())
    }

    /// Pushes a value to the end of the vector.
    pub fn push<Vq, ReaderAndWriter>(
        &self,
        value: &Vq,
        state: &mut ReaderAndWriter,
    ) -> Result<(), <ReaderAndWriter as StateWriter<N>>::Error>
    where
        Vq: ?Sized,
        Codec::ValueCodec: EncodeLike<Vq, V>,
        ReaderAndWriter: StateReaderAndWriter<N>,
    {
        let len = self.len(state)?;

        self.elems().set(&len, value, state)?;
        self.set_len(len + 1, state)?;

        Ok(())
    }

    /// Pops a value from the end of the vector and returns it.
    pub fn pop<ReaderAndWriter: StateReaderAndWriter<N>>(
        &self,
        state: &mut ReaderAndWriter,
    ) -> Result<Option<V>, <ReaderAndWriter as StateWriter<N>>::Error> {
        let len = self.len(state)?;
        let last_i = match len.checked_sub(1) {
            Some(i) => i,
            None => return Ok(None),
        };

        let elem = match self.elems().remove(&last_i, state)? {
            Some(elem) => elem,
            None => return Ok(None),
        };

        let new_len = last_i;
        self.set_len(new_len, state)?;

        Ok(Some(elem))
    }

    /// Removes the value at the specified index and returns it
    pub fn remove<ReaderAndWriter: StateReaderAndWriter<N>>(
        &self,
        index: usize,
        state: &mut ReaderAndWriter,
    ) -> Result<Option<V>, <ReaderAndWriter as StateWriter<N>>::Error> {
        let len = self.len(state)?;

        let new_len = match len.checked_sub(1) {
            Some(i) => i,
            None => return Ok(None),
        };

        Ok(if index < len {
            let elem = match self.elems().remove(&(index), state)? {
                Some(elem) => elem,
                None => return Ok(None),
            };

            for i in index..new_len {
                let next_elem = self.elems().remove(&(i + 1), state)?;
                if let Some(next_elem) = next_elem {
                    self.elems().set(&i, &next_elem, state)?;
                }
            }

            self.set_len(new_len, state)?;

            Some(elem)
        } else {
            None
        })
    }

    /// Removes all values from this vector.
    pub fn clear<ReaderAndWriter: StateReaderAndWriter<N>>(
        &self,
        state: &mut ReaderAndWriter,
    ) -> Result<(), <ReaderAndWriter as StateWriter<N>>::Error> {
        let len = self.len_value().remove(state)?.unwrap_or_default();

        for i in 0..len {
            self.elems().delete(&i, state)?;
        }

        Ok(())
    }

    /// Sets all values in the vector.
    ///
    /// If the length of the provided values is less than the length of the
    /// vector, the remaining values will be removed from storage.
    pub fn set_all<Vq, ReaderAndWriter>(
        &self,
        values: Vec<Vq>,
        state: &mut ReaderAndWriter,
    ) -> Result<(), <ReaderAndWriter as StateWriter<N>>::Error>
    where
        Codec::ValueCodec: EncodeLike<Vq, V>,
        ReaderAndWriter: StateReaderAndWriter<N>,
    {
        let old_len = self.len(state)?;
        let new_len = values.len();

        for i in new_len..old_len {
            self.elems().delete(&i, state)?;
        }

        for (i, value) in values.into_iter().enumerate() {
            self.elems().set(&i, &value, state)?;
        }

        self.set_len(new_len, state)
    }

    /// Returns the last value in the vector, or [`None`] if
    /// empty.
    pub fn last<ReaderAndWriter: StateReaderAndWriter<N>>(
        &self,
        state: &mut ReaderAndWriter,
    ) -> Result<Option<V>, <ReaderAndWriter as StateWriter<N>>::Error> {
        let len = self.len(state)?;
        let i = match len.checked_sub(1) {
            Some(i) => i,
            None => return Ok(None),
        };

        self.elems().get(&i, state)
    }

    /// Returns an iterator over all the values in the vector.
    pub fn iter<'a, 'ws, W>(
        &'a self,
        state: &'ws mut W,
    ) -> Result<StateVecIter<'a, 'ws, N, V, Codec, W>, <W as StateWriter<N>>::Error>
    where
        W: StateReaderAndWriter<N>,
    {
        let len = self.len(state)?;
        Ok(StateVecIter {
            state_vec: self,
            state,
            len,
            next_i: 0,
            _phantom: Default::default(),
        })
    }

    /// Collects all items returned by [`StateVec::iter`] into a collection. Only available with
    /// [`ApiStateAccessor`](crate::ApiStateAccessor) and other infallible state accessors.
    pub fn collect_infallible<B, W>(&self, state: &mut W) -> B
    where
        B: FromIterator<V>,
        W: InfallibleStateReaderAndWriter<N>,
    {
        self.iter(state)
            .unwrap_infallible()
            .map(|res| res.unwrap_infallible())
            .collect()
    }
}

/// An [`Iterator`] over a state vector.
///
/// See [`NamespacedStateVec::iter`] for more details.
pub struct StateVecIter<'a, 'ws, N, V, Codec, W>
where
    Codec: StateCodec,
    Codec::ValueCodec: StateItemCodec<V> + StateItemCodec<usize>,
    Codec::KeyCodec: StateItemCodec<usize>,
    N: CompileTimeNamespace,
    W: StateReaderAndWriter<N>,
{
    state_vec: &'a NamespacedStateVec<N, V, Codec>,
    state: &'ws mut W,
    len: usize,
    next_i: usize,
    _phantom: std::marker::PhantomData<(N, V, Codec)>,
}

impl<'a, 'ws, N, V, Codec, W> Iterator for StateVecIter<'a, 'ws, N, V, Codec, W>
where
    Codec: StateCodec,
    Codec::ValueCodec: StateItemCodec<V> + StateItemCodec<usize>,
    Codec::KeyCodec: StateItemCodec<usize>,
    N: CompileTimeNamespace,
    W: StateReaderAndWriter<N>,
{
    type Item = Result<V, <W as StateWriter<N>>::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.state_vec.get(self.next_i, self.state) {
            Err(e) => Some(Err(e)),
            Ok(None) => None,
            Ok(Some(elem)) => {
                self.next_i += 1;
                Some(Ok(elem))
            }
        }
    }
}

impl<'a, 'ws, N, V, Codec, W> ExactSizeIterator for StateVecIter<'a, 'ws, N, V, Codec, W>
where
    Codec: StateCodec,
    Codec::ValueCodec: StateItemCodec<V> + StateItemCodec<usize>,
    Codec::KeyCodec: StateItemCodec<usize>,
    N: CompileTimeNamespace,
    W: InfallibleStateReaderAndWriter<N>,
{
    fn len(&self) -> usize {
        self.len - self.next_i
    }
}

impl<'a, 'ws, N, V, Codec, W> FusedIterator for StateVecIter<'a, 'ws, N, V, Codec, W>
where
    Codec: StateCodec,
    Codec::ValueCodec: StateItemCodec<V> + StateItemCodec<usize>,
    Codec::KeyCodec: StateItemCodec<usize>,
    N: CompileTimeNamespace,
    W: InfallibleStateReaderAndWriter<N>,
{
}

impl<'a, 'ws, N, V, Codec, W> DoubleEndedIterator for StateVecIter<'a, 'ws, N, V, Codec, W>
where
    Codec: StateCodec,
    Codec::ValueCodec: StateItemCodec<V> + StateItemCodec<usize>,
    Codec::KeyCodec: StateItemCodec<usize>,
    N: CompileTimeNamespace,
    W: StateReaderAndWriter<N>,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.len == 0 {
            return None;
        }

        self.len -= 1;
        self.state_vec.get(self.len, self.state).transpose()
    }
}

#[cfg(all(test, feature = "native"))]
mod test {
    use std::fmt::Debug;

    use sov_mock_zkvm::MockZkVerifier;
    use sov_rollup_interface::execution_mode::Native;
    use sov_state::codec::BorshCodec;
    use sov_state::Prefix;
    use sov_test_utils::storage::new_finalized_storage;
    use unwrap_infallible::UnwrapInfallible;

    use super::*;
    use crate::capabilities::mocks::MockKernel;
    use crate::{Spec, StateCheckpoint};

    type TestSpec = crate::default_spec::DefaultSpec<MockZkVerifier, MockZkVerifier, Native>;

    #[test]
    fn test_state_vec() {
        let tmpdir = tempfile::tempdir().unwrap();
        let storage = new_finalized_storage(tmpdir.path());
        let mut state: StateCheckpoint<<TestSpec as Spec>::Storage> =
            StateCheckpoint::new(storage, &MockKernel::<TestSpec>::default());

        let prefix = Prefix::new("test".as_bytes().to_vec());
        let state_vec = StateVec::<u32>::new(prefix);

        for test_case_action in test_cases() {
            check_test_case_action(&state_vec, test_case_action, &mut state);
        }
    }
    enum TestCaseAction<T> {
        Push(T),
        Pop(T),
        Remove(usize, T),
        Last(T),
        Set(usize, T),
        SetAll(Vec<T>),
        CheckLen(usize),
        CheckContents(Vec<T>),
        CheckContentsReverse(Vec<T>),
        CheckGet(usize, Option<T>),
        Clear,
    }

    fn test_cases() -> Vec<TestCaseAction<u32>> {
        vec![
            TestCaseAction::Push(1),
            TestCaseAction::Push(2),
            TestCaseAction::CheckContents(vec![1, 2]),
            TestCaseAction::CheckLen(2),
            TestCaseAction::Pop(2),
            TestCaseAction::Set(0, 10),
            TestCaseAction::CheckContents(vec![10]),
            TestCaseAction::Push(8),
            TestCaseAction::CheckContents(vec![10, 8]),
            TestCaseAction::SetAll(vec![10]),
            TestCaseAction::CheckContents(vec![10]),
            TestCaseAction::CheckGet(1, None),
            TestCaseAction::Set(0, u32::MAX),
            TestCaseAction::Push(8),
            TestCaseAction::Push(0),
            TestCaseAction::CheckContents(vec![u32::MAX, 8, 0]),
            TestCaseAction::SetAll(vec![11, 12]),
            TestCaseAction::CheckContents(vec![11, 12]),
            TestCaseAction::SetAll(vec![]),
            TestCaseAction::CheckLen(0),
            TestCaseAction::Push(42),
            TestCaseAction::Push(1337),
            TestCaseAction::Clear,
            TestCaseAction::CheckContents(vec![]),
            TestCaseAction::CheckGet(0, None),
            TestCaseAction::SetAll(vec![1, 2, 3]),
            TestCaseAction::CheckContents(vec![1, 2, 3]),
            TestCaseAction::CheckContentsReverse(vec![3, 2, 1]),
            TestCaseAction::Last(3),
            TestCaseAction::Remove(0, 1),
            TestCaseAction::CheckContents(vec![2, 3]),
        ]
    }

    fn check_test_case_action<N, T, W>(
        state_vec: &NamespacedStateVec<N, T>,
        action: TestCaseAction<T>,
        state: &mut W,
    ) where
        BorshCodec: StateItemCodec<T>,
        T: Eq + Debug,
        W: InfallibleStateReaderAndWriter<N>,
        N: CompileTimeNamespace,
    {
        match action {
            TestCaseAction::CheckContents(expected) => {
                let contents: Vec<T> = state_vec.collect_infallible(state);
                assert_eq!(expected, contents);
            }
            TestCaseAction::CheckLen(expected) => {
                let actual = state_vec.len(state).unwrap_infallible();
                assert_eq!(actual, expected);
            }
            TestCaseAction::Pop(expected) => {
                let actual = state_vec.pop(state).unwrap_infallible();
                assert_eq!(actual, Some(expected));
            }
            TestCaseAction::Push(value) => {
                state_vec.push(&value, state).unwrap_infallible();
            }
            TestCaseAction::Set(index, value) => {
                state_vec
                    .set(index, &value, state)
                    .unwrap_infallible()
                    .unwrap();
            }
            TestCaseAction::SetAll(values) => {
                state_vec.set_all(values, state).unwrap_infallible();
            }
            TestCaseAction::CheckGet(index, expected) => {
                let actual = state_vec.get(index, state).unwrap_infallible();
                assert_eq!(actual, expected);
            }
            TestCaseAction::Clear => {
                state_vec.clear(state).unwrap_infallible();
            }
            TestCaseAction::Last(expected) => {
                let actual = state_vec.last(state).unwrap_infallible();
                assert_eq!(actual, Some(expected));
            }
            TestCaseAction::CheckContentsReverse(expected) => {
                let mut contents = state_vec.collect_infallible::<Vec<T>, _>(state);
                contents.reverse();
                assert_eq!(expected, contents);
            }
            TestCaseAction::Remove(index, expected) => {
                let actual = state_vec.remove(index, state).unwrap_infallible();
                assert_eq!(actual, Some(expected));
            }
        }
    }
}
