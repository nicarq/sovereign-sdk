use std::convert::Infallible;
use std::iter::FusedIterator;
use std::marker::PhantomData;

use sov_state::codec::BorshCodec;
use sov_state::namespaces::{CompileTimeNamespace, Kernel};
use sov_state::{EncodeLike, Prefix, StateCodec, StateItemCodec, Storage};
use thiserror::Error;
use unwrap_infallible::UnwrapInfallible;

use super::map::NamespacedStateMap;
use super::{KernelStateValue, VersionedStateValue};
use crate::{
    InfallibleStateReaderAndWriter, KernelStateAccessor, KernelWriter, StateReader, VersionReader,
};

/// A growable array of values stored as JMT-backed state. This is the versioned version of [`crate::StateVec`].
/// There are a few differences with the non-versioned version:
/// - The values are systematically stored in the [`Kernel`] namespace.
/// - The length of the vector is stored as a [`VersionedStateValue`], which is a versioned value. This allows us
/// to have a growable vector of values that depends on the current version of the rollup (which is compatible with
/// soft-confirmations).
/// - The data structure is *append-only*. This means that the vector can only be modified by appending new values to the end of the vector.
/// The last element in the vector can be modified using the [`VersionedStateVec::set_last`] method.
/// This choice is motivated by the fact that the vector is used in the soft-confirmations context, so there shouldn't be
/// any way to modify older keys from the vector without breaking the soft-confirmations mechanism.
/// - This data structure *needs* to be initialized at genesis. Otherwise, the state vector will be in an invalid state.
#[derive(
    Debug,
    Clone,
    PartialEq,
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct VersionedStateVec<V, Codec = BorshCodec> {
    _phantom: PhantomData<V>,
    pub(crate) prefix: Prefix,
    pub(crate) len_value: VersionedStateValue<u64, Codec>,
    // The maximum length index is used to determine the maximum index at which the `len_value` map is defined.
    pub(crate) max_len_index: KernelStateValue<u64, Codec>,
    pub(crate) elems: NamespacedStateMap<Kernel, u64, V, Codec>,
}

/// An error type for vector getters.
#[derive(Debug, Error)]
pub enum VersionedStateVecError {
    /// Operation failed because the index was out of bounds.
    #[error("Index out of bounds for index: {index} with kernel namespace. Current vector length {length}")]
    IndexOutOfBounds { index: u64, length: u64 },
    /// Value not found.
    #[error("Value not found for prefix: {prefix} and index: {index} with kernel namespace")]
    MissingValue { prefix: Prefix, index: u64 },
}

type VersionedStateVecResult<V> = Result<V, VersionedStateVecError>;

impl<V> VersionedStateVec<V>
where
    BorshCodec: StateItemCodec<V>,
{
    /// Crates a new [`crate::StateVec`] with the given prefix and the default
    /// [`StateItemCodec`] (i.e. [`BorshCodec`]).
    pub fn new(prefix: Prefix) -> Self {
        Self::with_codec(prefix, BorshCodec)
    }
}

impl<V, Codec: Clone> VersionedStateVec<V, Codec>
where
    Codec: StateCodec,
    Codec::ValueCodec: StateItemCodec<V> + StateItemCodec<u64>,
    Codec::KeyCodec: StateItemCodec<u64>,
{
    /// Creates a new [`crate::StateVec`] with the given prefix and codec.
    pub fn with_codec(prefix: Prefix, codec: Codec) -> Self {
        // Differentiating the prefixes for the length and the elements
        // shouldn't be necessary, but it's best not to rely on implementation
        // details of `StateValue` and `StateMap` as they both have the right to
        // reserve the whole key space for themselves.
        let len_value = VersionedStateValue::with_codec(prefix.extended(b"l"), codec.clone());
        let elems = NamespacedStateMap::with_codec(prefix.extended(b"e"), codec.clone());
        let max_len_index = KernelStateValue::with_codec(prefix.extended(b"m"), codec);
        Self {
            _phantom: PhantomData,
            prefix,
            len_value,
            max_len_index,
            elems,
        }
    }

    /// Initializes the state vector by setting the length to zero.
    ///
    /// ## Warning
    /// This step *needs* to be done before any other operation on the state vector to ensure that the state vector is in a valid state.
    pub fn initialize(&self, state: &mut impl KernelWriter) {
        self.len_value.set_true_current(&0, state);
    }

    /// Returns the prefix used when this [`crate::StateVec`] was created.
    pub fn prefix(&self) -> &Prefix {
        &self.prefix
    }

    fn set_true_len(&self, length: u64, state: &mut impl KernelWriter) {
        self.len_value.set_true_current(&length, state);
    }

    fn elems(&self) -> &NamespacedStateMap<Kernel, u64, V, Codec> {
        &self.elems
    }

    fn len_value(&self) -> &VersionedStateValue<u64, Codec> {
        &self.len_value
    }

    /// Returns the value for the given index.
    pub fn get<Reader: VersionReader>(
        &self,
        index: u64,
        state: &mut Reader,
    ) -> Result<Option<V>, <Reader as StateReader<Kernel>>::Error> {
        let len = self.len(state)?;

        if index < len {
            self.elems().get(&index, state)
        } else {
            Ok(None)
        }
    }

    /// Returns the value for the given index.
    /// If the index is out of bounds, returns an error.
    /// If the value is absent, returns an error.
    pub fn get_or_err<Reader: VersionReader>(
        &self,
        index: u64,
        state: &mut Reader,
    ) -> Result<VersionedStateVecResult<V>, <Reader as StateReader<Kernel>>::Error> {
        let len = self.len(state)?;

        Ok(if index < len {
            self.elems()
                .get(&index, state)?
                .ok_or_else(|| VersionedStateVecError::MissingValue {
                    prefix: self.prefix().clone(),
                    index,
                })
        } else {
            Err(VersionedStateVecError::IndexOutOfBounds { index, length: len })
        })
    }

    /// Returns the current length of the vector. Ie, the length of the vector at the version visible from the accessor.
    ///
    /// ## Note
    /// If the current height to access is greater than the maximum length stored in `len_value`, we will return
    /// `len_value[max_len_index]` instead of `None`. This is safe to do because the [`VersionedStateVec`] is an _append-only_ data structure,
    /// and hence querying the values at indexes below `len_value[max_len_index]` will always return the same value for future heights.
    /// Also note that if the current height to access is less than `max_len_index`, we will naturally return `len_value[max_len_index]`.
    pub fn len<Reader: VersionReader>(
        &self,
        state: &mut Reader,
    ) -> Result<u64, <Reader as StateReader<Kernel>>::Error> {
        if let Some(len_index) = self.max_len_index.get(state)? {
            // If the current height to access is greater than the maximum length index, we can use the length at the maximum length index.
            // Otherwise, we can use the length at the current height to access.
            if state.rollup_height_to_access() > len_index {
                return Ok(self
                    .len_value()
                    .get(&len_index, state)?
                    .expect("The length should always be defined at the maximum length index"));
            } else {
                return Ok(self.len_value().get_current(state)?.expect("All the values of the vector located at indexes below `max_len_index` should be defined"));
            }
        }

        // If the `max_len_index` is not set, this means that the vector is empty.
        Ok(0)
    }

    /// Pushes a value to the end of the vector. This operation should be performed by a [`KernelStateAccessor`].
    pub fn push<Vq, Accessor: KernelWriter + VersionReader<Error = Infallible>>(
        &self,
        value: &Vq,
        state: &mut Accessor,
    ) where
        Vq: ?Sized,
        Codec::ValueCodec: EncodeLike<Vq, V>,
    {
        let len = self.len(state).unwrap_infallible();
        self.elems().set(&len, value, state).unwrap_infallible();
        self.set_true_len(len + 1, state);

        self.max_len_index
            .set(&state.true_rollup_height(), state)
            .unwrap_infallible();
    }

    /// Returns the last value in the vector at the version visible from the accessor, or [`None`] if
    /// empty.
    pub fn last<VersionedState: VersionReader>(
        &self,
        state: &mut VersionedState,
    ) -> Result<Option<V>, VersionedState::Error> {
        let len = self.len(state)?;

        let i = match len.checked_sub(1) {
            Some(i) => i,
            None => return Ok(None),
        };

        self.elems().get(&i, state)
    }

    /// Sets the last element in a versioned state vector. Returns an error if the vector is empty.
    pub fn set_last<Vq, S: Storage>(
        &self,
        new_value: &Vq,
        state: &mut KernelStateAccessor<S>,
    ) -> Result<(), anyhow::Error>
    where
        Vq: ?Sized,
        Codec::ValueCodec: EncodeLike<Vq, V>,
    {
        let len = self.len(state)?;
        let i = match len.checked_sub(1) {
            Some(i) => i,
            None => anyhow::bail!("Vector is empty, impossible to set last element!"),
        };

        self.elems().set(&i, new_value, state).unwrap_infallible();

        Ok(())
    }

    /// Returns an iterator over all the values in the vector.
    pub fn iter<'a, 'ws, W>(
        &'a self,
        state: &'ws mut W,
    ) -> Result<VersionedStateVecIter<'a, 'ws, Kernel, V, Codec, W>, W::Error>
    where
        W: VersionReader,
    {
        let len = self.len(state)?;
        Ok(VersionedStateVecIter {
            state_vec: self,
            state,
            len,
            next_i: 0,
            _phantom: Default::default(),
        })
    }

    /// Collects all items returned by [`VersionedStateVec::iter`] into a collection. Only available with
    /// [`ApiStateAccessor`](crate::ApiStateAccessor) and other infallible state accessors.
    pub fn collect_infallible<B, W>(&self, state: &mut W) -> B
    where
        B: FromIterator<V>,
        W: InfallibleStateReaderAndWriter<Kernel> + VersionReader,
    {
        self.iter(state)
            .unwrap_infallible()
            .map(|res| res.unwrap_infallible())
            .collect()
    }
}

/// An [`Iterator`] over a state vector.
///
/// See [`VersionedStateVec::iter`] for more details.
pub struct VersionedStateVecIter<'a, 'ws, Kernel, V, Codec, W>
where
    Codec: StateCodec,
    Codec::ValueCodec: StateItemCodec<V>,
    Codec::KeyCodec: StateItemCodec<u64>,
    Kernel: CompileTimeNamespace,
    W: VersionReader,
{
    state_vec: &'a VersionedStateVec<V, Codec>,
    state: &'ws mut W,
    len: u64,
    next_i: u64,
    _phantom: std::marker::PhantomData<(Kernel, V, Codec)>,
}

impl<'a, 'ws, V, Codec, W> Iterator for VersionedStateVecIter<'a, 'ws, Kernel, V, Codec, W>
where
    Codec: StateCodec,
    Codec::ValueCodec: StateItemCodec<V> + StateItemCodec<u64>,
    Codec::KeyCodec: StateItemCodec<u64>,
    W: VersionReader,
{
    type Item = Result<V, W::Error>;

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

impl<'a, 'ws, V, Codec, W> ExactSizeIterator for VersionedStateVecIter<'a, 'ws, Kernel, V, Codec, W>
where
    Codec: StateCodec,
    Codec::ValueCodec: StateItemCodec<V> + StateItemCodec<u64>,
    Codec::KeyCodec: StateItemCodec<u64>,
    W: VersionReader + InfallibleStateReaderAndWriter<Kernel>,
{
    fn len(&self) -> usize {
        (self.len - self.next_i) as usize
    }
}

impl<'a, 'ws, V, Codec, W> FusedIterator for VersionedStateVecIter<'a, 'ws, Kernel, V, Codec, W>
where
    Codec: StateCodec,
    Codec::ValueCodec: StateItemCodec<V> + StateItemCodec<u64>,
    Codec::KeyCodec: StateItemCodec<u64>,
    W: VersionReader + InfallibleStateReaderAndWriter<Kernel>,
{
}

impl<'a, 'ws, V, Codec, W> DoubleEndedIterator
    for VersionedStateVecIter<'a, 'ws, Kernel, V, Codec, W>
where
    Codec: StateCodec,
    Codec::ValueCodec: StateItemCodec<V> + StateItemCodec<u64>,
    Codec::KeyCodec: StateItemCodec<u64>,
    W: VersionReader,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.len == 0 {
            return None;
        }

        self.len -= 1;
        self.state_vec.get(self.len, self.state).transpose()
    }
}

#[cfg(test)]
mod test {
    use std::fmt::Debug;

    use sov_mock_zkvm::MockZkvm;
    use sov_rollup_interface::execution_mode::Native;
    use sov_state::codec::BorshCodec;
    use sov_state::Prefix;
    use sov_test_utils::storage::new_finalized_storage;
    use sov_test_utils::MockDaSpec;
    use unwrap_infallible::UnwrapInfallible;

    use super::*;
    use crate::capabilities::mocks::MockKernel;
    use crate::capabilities::Kernel as _;
    use crate::{Spec, StateCheckpoint};

    type TestSpec = crate::default_spec::DefaultSpec<MockDaSpec, MockZkvm, MockZkvm, Native>;

    #[test]
    fn test_state_vec() {
        let tmpdir = tempfile::tempdir().unwrap();
        let storage = new_finalized_storage(tmpdir.path());
        let kernel = MockKernel::<TestSpec>::default();
        let mut state: StateCheckpoint<<TestSpec as Spec>::Storage> =
            StateCheckpoint::new(storage, &kernel);

        let prefix = Prefix::new("test".as_bytes().to_vec());
        let state_vec = VersionedStateVec::<u32>::new(prefix);

        // We need to initialize the state vector before we can run any test case.
        state_vec.initialize(&mut kernel.accessor(&mut state));

        let mut kernel = MockKernel::<TestSpec>::default();
        kernel.true_rollup_height = 0;

        test_cases().into_iter().for_each(|test_case_action| {
            check_test_case_action(&state_vec, test_case_action, &mut kernel, &mut state);
        });
    }

    #[derive(Debug)]
    enum TestCaseAction<T> {
        ExtendAndPush(T),
        Push(T),
        Last(T),
        SetLast(T),
        CheckLen(u64),
        CheckContents(Vec<T>),
        CheckContentsReverse(Vec<T>),
        CheckGet(u64, Option<T>),
        // This is a special case for the soft-confirmations mechanism,
        // it increases the virtual height of the kernel.
        IncreaseVirtualHeight,
        // This is a special case for the soft-confirmations mechanism,
        // it checks that the true rollup height and the virtual rollup height are updated correctly.
        CheckHeights {
            true_slot_num: u64,
            virtual_slot_num: u64,
        },
    }

    fn test_cases() -> Vec<TestCaseAction<u32>> {
        vec![
            TestCaseAction::CheckLen(0),
            TestCaseAction::CheckContents(vec![]),
            TestCaseAction::ExtendAndPush(1),
            TestCaseAction::CheckHeights {
                true_slot_num: 1,
                virtual_slot_num: 0,
            },
            TestCaseAction::CheckLen(0),
            TestCaseAction::ExtendAndPush(2),
            TestCaseAction::CheckHeights {
                true_slot_num: 2,
                virtual_slot_num: 0,
            },
            TestCaseAction::CheckLen(0),
            TestCaseAction::CheckContents(vec![]),
            TestCaseAction::CheckGet(0, None),
            TestCaseAction::CheckGet(1, None),
            TestCaseAction::IncreaseVirtualHeight,
            TestCaseAction::CheckHeights {
                true_slot_num: 2,
                virtual_slot_num: 1,
            },
            TestCaseAction::CheckContents(vec![1]),
            TestCaseAction::CheckLen(1),
            TestCaseAction::IncreaseVirtualHeight,
            TestCaseAction::CheckContents(vec![1, 2]),
            TestCaseAction::CheckLen(2),
            TestCaseAction::ExtendAndPush(8),
            TestCaseAction::CheckHeights {
                true_slot_num: 3,
                virtual_slot_num: 2,
            },
            TestCaseAction::CheckContents(vec![1, 2]),
            TestCaseAction::CheckLen(2),
            TestCaseAction::IncreaseVirtualHeight,
            TestCaseAction::CheckContents(vec![1, 2, 8]),
            TestCaseAction::CheckLen(3),
            TestCaseAction::CheckGet(0, Some(1)),
            TestCaseAction::CheckGet(2, Some(8)),
            TestCaseAction::ExtendAndPush(8),
            TestCaseAction::ExtendAndPush(0),
            TestCaseAction::CheckContents(vec![1, 2, 8]),
            TestCaseAction::CheckContentsReverse(vec![8, 2, 1]),
            TestCaseAction::Last(8),
            TestCaseAction::CheckGet(4, None),
            TestCaseAction::CheckHeights {
                true_slot_num: 5,
                virtual_slot_num: 3,
            },
            TestCaseAction::CheckLen(3),
            TestCaseAction::IncreaseVirtualHeight,
            TestCaseAction::IncreaseVirtualHeight,
            TestCaseAction::CheckContents(vec![1, 2, 8, 8, 0]),
            TestCaseAction::Last(0),
            TestCaseAction::CheckContentsReverse(vec![0, 8, 8, 2, 1]),
            TestCaseAction::CheckLen(5),
            TestCaseAction::Push(10),
            TestCaseAction::Push(15),
            TestCaseAction::CheckLen(7),
            TestCaseAction::CheckContents(vec![1, 2, 8, 8, 0, 10, 15]),
            TestCaseAction::SetLast(11),
            TestCaseAction::CheckContents(vec![1, 2, 8, 8, 0, 10, 11]),
        ]
    }

    fn check_test_case_action<T, S: Spec>(
        state_vec: &VersionedStateVec<T>,
        action: TestCaseAction<T>,
        kernel: &mut MockKernel<S>,
        state: &mut StateCheckpoint<S::Storage>,
    ) where
        BorshCodec: StateItemCodec<T>,
        T: Eq + Debug,
    {
        // For some of the test cases we convert the state to the versioned state reader at virtual height to
        // be able to simulate what happens in soft-confirmations context.
        match action {
            TestCaseAction::CheckContents(expected) => {
                let contents: Vec<T> = state_vec.collect_infallible(state);
                assert_eq!(contents, expected);
            }
            TestCaseAction::CheckLen(expected) => {
                let actual = state_vec.len(state).unwrap_infallible();
                assert_eq!(actual, expected);
            }
            TestCaseAction::ExtendAndPush(value) => {
                kernel.true_rollup_height += 1;
                let state = &mut KernelStateAccessor::from_checkpoint(kernel, state);
                state_vec.push(&value, state);
            }
            TestCaseAction::Push(value) => {
                let state = &mut KernelStateAccessor::from_checkpoint(kernel, state);
                state_vec.push(&value, state);
            }
            TestCaseAction::CheckGet(index, expected) => {
                let actual = state_vec.get(index, state).unwrap_infallible();
                assert_eq!(actual, expected);
            }
            TestCaseAction::Last(expected) => {
                let actual = state_vec.last(state).unwrap_infallible();
                assert_eq!(actual, Some(expected));
            }
            TestCaseAction::SetLast(value) => {
                let state = &mut KernelStateAccessor::from_checkpoint(kernel, state);
                state_vec.set_last(&value, state).unwrap();
            }
            TestCaseAction::CheckContentsReverse(expected) => {
                let mut contents = state_vec.collect_infallible::<Vec<T>, _>(state);
                contents.reverse();
                assert_eq!(contents, expected);
            }
            TestCaseAction::IncreaseVirtualHeight => {
                state.update_version(state.rollup_height_to_access() + 1);
            }
            TestCaseAction::CheckHeights {
                true_slot_num,
                virtual_slot_num,
            } => {
                let state = &mut KernelStateAccessor::from_checkpoint(kernel, state);
                assert_eq!(state.rollup_height_to_access(), true_slot_num);
                assert_eq!(state.visible_rollup_height(), virtual_slot_num);
            }
        }
    }
}
