use std::marker::PhantomData;

use sov_modules_core::namespaces::{CompileTimeNamespace, User};
use sov_modules_core::{Namespace, Prefix};
#[cfg(feature = "arbitrary")]
use sov_modules_core::{StateReaderAndWriter, WorkingSet};
use sov_state::codec::BorshCodec;

#[cfg(feature = "arbitrary")]
use crate::StateMapAccessor;
/// A container that maps keys to values.
///
/// # Type parameters
/// [`StateMap`] is generic over:
/// - a key type `K`;
/// - a value type `V`;
/// - a [`sov_modules_core::StateValueCodec`] `Codec`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct GenericStateMap<N, K, V, Codec = BorshCodec> {
    _phantom: PhantomData<(N, K, V)>,
    pub(crate) codec: Codec,
    pub(crate) prefix: Prefix,
}

pub type StateMap<K, V, Codec = BorshCodec> = GenericStateMap<User, K, V, Codec>;

impl<N: CompileTimeNamespace, K, V> GenericStateMap<N, K, V> {
    /// Creates a new [`StateMap`] with the given prefix and the default
    /// [`sov_modules_core::StateValueCodec`] (i.e. [`BorshCodec`]).
    pub fn new(prefix: Prefix) -> Self {
        Self::with_codec(prefix, BorshCodec)
    }
}

impl<N: CompileTimeNamespace, K, V, Codec> GenericStateMap<N, K, V, Codec> {
    /// Creates a new [`StateMap`] with the given prefix and [`sov_modules_core::StateValueCodec`].
    pub fn with_codec(prefix: Prefix, codec: Codec) -> Self {
        Self {
            _phantom: PhantomData,
            codec,
            prefix,
        }
    }

    pub fn namespace(&self) -> Namespace {
        N::NAMESPACE
    }

    /// Returns a reference to the codec used by this [`StateMap`].
    pub fn codec(&self) -> &Codec {
        &self.codec
    }

    /// Returns the prefix used when this [`StateMap`] was created.
    pub fn prefix(&self) -> &Prefix {
        &self.prefix
    }
}

#[cfg(feature = "arbitrary")]
impl<'a, N, K, V, Codec> GenericStateMap<N, K, V, Codec>
where
    K: arbitrary::Arbitrary<'a>,
    V: arbitrary::Arbitrary<'a>,
    Codec: sov_modules_core::StateCodec + Default,
    Codec::KeyCodec: sov_modules_core::StateKeyCodec<K>,
    Codec::ValueCodec: sov_modules_core::StateValueCodec<V>,
    N: CompileTimeNamespace,
{
    /// Returns an arbitrary [`StateMap`] instance.
    ///
    /// See the [`arbitrary`] crate for more information.
    pub fn arbitrary_working_set<S>(
        u: &mut arbitrary::Unstructured<'a>,
        working_set: &mut sov_modules_core::WorkingSet<S>,
    ) -> arbitrary::Result<Self>
    where
        S: sov_modules_core::Spec,
        WorkingSet<S>: StateReaderAndWriter<N>,
    {
        use arbitrary::Arbitrary;

        let prefix = Prefix::arbitrary(u)?;
        let len = u.arbitrary_len::<(K, V)>()?;
        let codec = Codec::default();
        let map = Self::with_codec(prefix, codec);

        (0..len).try_fold(map, |map, _| {
            let key = K::arbitrary(u)?;
            let value = V::arbitrary(u)?;

            map.set(&key, &value, working_set);

            Ok(map)
        })
    }
}
