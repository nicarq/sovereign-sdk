use std::marker::PhantomData;

use sov_modules_core::{Prefix, StateValueCodec};
use sov_state::codec::BorshCodec;

use crate::containers::{StateMap, StateValue};

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
pub struct StateVec<V, Codec = BorshCodec> {
    _phantom: PhantomData<V>,
    pub(crate) prefix: Prefix,
    pub(crate) len_value: StateValue<usize, Codec>,
    pub(crate) elems: StateMap<usize, V, Codec>,
}

impl<V, Codec: Clone> StateVec<V, Codec> {
    /// Creates a new [`StateVec`] with the given prefix and codec.
    pub fn with_codec(prefix: Prefix, codec: Codec) -> Self {
        // Differentiating the prefixes for the length and the elements
        // shouldn't be necessary, but it's best not to rely on implementation
        // details of `StateValue` and `StateMap` as they both have the right to
        // reserve the whole key space for themselves.
        let len_value = StateValue::with_codec(prefix.extended(b"l"), codec.clone());
        let elems = StateMap::with_codec(prefix.extended(b"e"), codec);
        Self {
            _phantom: PhantomData,
            prefix,
            len_value,
            elems,
        }
    }
}

impl<V> StateVec<V>
where
    BorshCodec: StateValueCodec<V>,
{
    /// Crates a new [`StateVec`] with the given prefix and the default
    /// [`StateValueCodec`] (i.e. [`BorshCodec`]).
    pub fn new(prefix: Prefix) -> Self {
        Self::with_codec(prefix, BorshCodec)
    }
}

#[cfg(all(test, feature = "native"))]
mod test {
    use sov_modules_core::{Prefix, WorkingSet};
    use sov_prover_storage_manager::new_orphan_storage;
    use sov_test_utils::TestSpec;

    use crate::containers::traits::vec_tests::Testable;
    use crate::StateVec;

    #[test]
    fn test_state_vec() {
        let tmpdir = tempfile::tempdir().unwrap();
        let storage = new_orphan_storage(tmpdir.path()).unwrap();
        let mut working_set: WorkingSet<TestSpec> = WorkingSet::new(storage);

        let prefix = Prefix::new("test".as_bytes().to_vec());
        let state_vec = StateVec::<u32>::new(prefix);

        state_vec.run_tests(&mut working_set);
    }
}
