use std::iter::FusedIterator;
use std::marker::PhantomData;

use sov_modules_core::namespaces::{Accessory, CompileTimeNamespace, Kernel, User};
use sov_modules_core::{Prefix, StateCodec, StateItemCodec, StateReaderAndWriter};
use sov_state::codec::BorshCodec;
use thiserror::Error;

use super::map::NamespacedStateMap;
use super::value::NamespacedStateValue;

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

pub type StateVec<V, Codec = BorshCodec> = NamespacedStateVec<User, V, Codec>;
pub type AccessoryStateVec<V, Codec = BorshCodec> = NamespacedStateVec<Accessory, V, Codec>;
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
    Codec: StateCodec + Clone,
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

    fn set_len(&self, length: usize, working_set: &mut impl StateReaderAndWriter<N>) {
        self.len_value.set(&length, working_set);
    }

    fn elems(&self) -> &NamespacedStateMap<N, usize, V, Codec> {
        &self.elems
    }

    fn len_value(&self) -> &NamespacedStateValue<N, usize, Codec> {
        &self.len_value
    }

    /// Returns the prefix used when this [`StateVec`] was created.
    fn prefix(&self) -> &Prefix {
        &self.prefix
    }

    /// Sets a value in the vector.
    /// If the index is out of bounds, returns an error.
    /// To push a value to the end of the StateVec, use [`NamespacedStateVec::push`].
    pub fn set(
        &self,
        index: usize,
        value: &V,
        working_set: &mut impl StateReaderAndWriter<N>,
    ) -> Result<(), StateVecError<N>> {
        let len = self.len(working_set);

        if index < len {
            self.elems().set(&index, value, working_set);
            Ok(())
        } else {
            Err(StateVecError::IndexOutOfBounds(index))
        }
    }

    /// Returns the value for the given index.
    pub fn get(&self, index: usize, working_set: &mut impl StateReaderAndWriter<N>) -> Option<V> {
        self.elems().get(&index, working_set)
    }

    /// Returns the value for the given index.
    /// If the index is out of bounds, returns an error.
    /// If the value is absent, returns an error.
    pub fn get_or_err(
        &self,
        index: usize,
        working_set: &mut impl StateReaderAndWriter<N>,
    ) -> Result<V, StateVecError<N>> {
        let len = self.len(working_set);

        if index < len {
            self.elems().get(&index, working_set).ok_or_else(|| {
                StateVecError::MissingValue(self.prefix().clone(), index, PhantomData)
            })
        } else {
            Err(StateVecError::IndexOutOfBounds(index))
        }
    }

    /// Returns the length of the vector.
    pub fn len(&self, working_set: &mut impl StateReaderAndWriter<N>) -> usize {
        self.len_value().get(working_set).unwrap_or_default()
    }

    /// Pushes a value to the end of the vector.
    pub fn push(&self, value: &V, working_set: &mut impl StateReaderAndWriter<N>) {
        let len = self.len(working_set);

        self.elems().set(&len, value, working_set);
        self.set_len(len + 1, working_set);
    }

    /// Pops a value from the end of the vector and returns it.
    pub fn pop(&self, working_set: &mut impl StateReaderAndWriter<N>) -> Option<V> {
        let len = self.len(working_set);
        let last_i = len.checked_sub(1)?;
        let elem = self.elems().remove(&last_i, working_set)?;

        let new_len = last_i;
        self.set_len(new_len, working_set);

        Some(elem)
    }

    /// Removes all values from this vector.
    pub fn clear(&self, working_set: &mut impl StateReaderAndWriter<N>) {
        let len = self.len_value().remove(working_set).unwrap_or_default();

        for i in 0..len {
            self.elems().delete(&i, working_set);
        }
    }

    /// Sets all values in the tector.
    ///
    /// If the length of the provided values is less than the length of the
    /// vector, the remaining values will be removed from storage.
    pub fn set_all(&self, values: Vec<V>, working_set: &mut impl StateReaderAndWriter<N>) {
        let old_len = self.len(working_set);
        let new_len = values.len();

        for i in new_len..old_len {
            self.elems().delete(&i, working_set);
        }

        for (i, value) in values.into_iter().enumerate() {
            self.elems().set(&i, &value, working_set);
        }

        self.set_len(new_len, working_set);
    }

    /// Returns an iterator over all the values in the vector.
    pub fn iter<'a, 'ws, W>(
        &'a self,
        working_set: &'ws mut W,
    ) -> StateVecIter<'a, 'ws, N, V, Codec, W>
    where
        W: StateReaderAndWriter<N>,
    {
        let len = self.len(working_set);
        StateVecIter {
            state_vec: self,
            ws: working_set,
            len,
            next_i: 0,
            _phantom: Default::default(),
        }
    }

    /// Returns the last value in the vector, or [`None`] if
    /// empty.
    pub fn last(&self, working_set: &mut impl StateReaderAndWriter<N>) -> Option<V> {
        let len = self.len(working_set);
        let i = len.checked_sub(1)?;
        self.elems().get(&i, working_set)
    }
}

/// An [`Iterator`] over a state vector.
///
/// See [`NamespacedStateVec::iter`] for more details.
pub struct StateVecIter<'a, 'ws, N, V, Codec, W>
where
    Codec: StateCodec + Clone,
    Codec::ValueCodec: StateItemCodec<V> + StateItemCodec<usize>,
    Codec::KeyCodec: StateItemCodec<usize>,
    N: CompileTimeNamespace,
    W: StateReaderAndWriter<N>,
{
    state_vec: &'a NamespacedStateVec<N, V, Codec>,
    ws: &'ws mut W,
    len: usize,
    next_i: usize,
    _phantom: std::marker::PhantomData<(N, V, Codec)>,
}

impl<'a, 'ws, N, V, Codec, W> Iterator for StateVecIter<'a, 'ws, N, V, Codec, W>
where
    Codec: StateCodec + Clone,
    Codec::ValueCodec: StateItemCodec<V> + StateItemCodec<usize>,
    Codec::KeyCodec: StateItemCodec<usize>,
    N: CompileTimeNamespace,
    W: StateReaderAndWriter<N>,
{
    type Item = V;

    fn next(&mut self) -> Option<Self::Item> {
        let elem = self.state_vec.get(self.next_i, self.ws);
        if elem.is_some() {
            self.next_i += 1;
        }

        elem
    }
}

impl<'a, 'ws, N, V, Codec, W> ExactSizeIterator for StateVecIter<'a, 'ws, N, V, Codec, W>
where
    Codec: StateCodec + Clone,
    Codec::ValueCodec: StateItemCodec<V> + StateItemCodec<usize>,
    Codec::KeyCodec: StateItemCodec<usize>,
    N: CompileTimeNamespace,
    W: StateReaderAndWriter<N>,
{
    fn len(&self) -> usize {
        self.len - self.next_i
    }
}

impl<'a, 'ws, N, V, Codec, W> FusedIterator for StateVecIter<'a, 'ws, N, V, Codec, W>
where
    Codec: StateCodec + Clone,
    Codec::ValueCodec: StateItemCodec<V> + StateItemCodec<usize>,
    Codec::KeyCodec: StateItemCodec<usize>,
    N: CompileTimeNamespace,
    W: StateReaderAndWriter<N>,
{
}

impl<'a, 'ws, N, V, Codec, W> DoubleEndedIterator for StateVecIter<'a, 'ws, N, V, Codec, W>
where
    Codec: StateCodec + Clone,
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
        self.state_vec.get(self.len, self.ws)
    }
}

#[cfg(all(test, feature = "native"))]
mod test {
    use std::fmt::Debug;

    use sov_modules_core::{Prefix, WorkingSet};
    use sov_prover_storage_manager::new_orphan_storage;
    use sov_state::codec::BorshCodec;
    use sov_test_utils::TestSpec;

    use super::*;

    #[test]
    fn test_state_vec() {
        let tmpdir = tempfile::tempdir().unwrap();
        let storage = new_orphan_storage(tmpdir.path()).unwrap();
        let mut working_set: WorkingSet<TestSpec> = WorkingSet::new(storage);

        let prefix = Prefix::new("test".as_bytes().to_vec());
        let state_vec = StateVec::<u32>::new(prefix);

        for test_case_action in test_cases() {
            check_test_case_action(&state_vec, test_case_action, &mut working_set);
        }
    }

    enum TestCaseAction<T> {
        Push(T),
        Pop(T),
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
        ]
    }

    fn check_test_case_action<N, T, W>(
        state_vec: &NamespacedStateVec<N, T>,
        action: TestCaseAction<T>,
        ws: &mut W,
    ) where
        BorshCodec: StateItemCodec<T>,
        T: Eq + Debug,
        W: StateReaderAndWriter<N>,
        N: CompileTimeNamespace,
    {
        match action {
            TestCaseAction::CheckContents(expected) => {
                let contents: Vec<T> = state_vec.iter(ws).collect();
                assert_eq!(expected, contents);
            }
            TestCaseAction::CheckLen(expected) => {
                let actual = state_vec.len(ws);
                assert_eq!(actual, expected);
            }
            TestCaseAction::Pop(expected) => {
                let actual = state_vec.pop(ws);
                assert_eq!(actual, Some(expected));
            }
            TestCaseAction::Push(value) => {
                state_vec.push(&value, ws);
            }
            TestCaseAction::Set(index, value) => {
                state_vec.set(index, &value, ws).unwrap();
            }
            TestCaseAction::SetAll(values) => {
                state_vec.set_all(values, ws);
            }
            TestCaseAction::CheckGet(index, expected) => {
                let actual = state_vec.get(index, ws);
                assert_eq!(actual, expected);
            }
            TestCaseAction::Clear => {
                state_vec.clear(ws);
            }
            TestCaseAction::Last(expected) => {
                let actual = state_vec.last(ws);
                assert_eq!(actual, Some(expected));
            }
            TestCaseAction::CheckContentsReverse(expected) => {
                let contents: Vec<T> = state_vec.iter(ws).rev().collect();
                assert_eq!(expected, contents);
            }
        }
    }
}
