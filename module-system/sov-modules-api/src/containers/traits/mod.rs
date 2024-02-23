mod map;
mod value;
mod vec;
pub use map::{StateMapAccessor, StateMapError};
use sov_modules_core::{
    Namespace, Prefix, StateCheckpoint, StateCodec, StateKeyCodec, StateReaderAndWriter,
    StateValueCodec, WorkingSet,
};
pub use value::{StateValueAccessor, StateValueError};
#[cfg(test)]
pub use vec::tests as vec_tests;
pub use vec::{StateVecAccessor, StateVecError, StateVecPrivateAccessor};

use crate::{Spec, StateMap, StateValue, StateVec};

/// A type that can both read and write the normal "user-space" state of the rollup.
///
/// ```
/// use sov_modules_api::StateValueAccessor;
/// fn delete_state_string(value: sov_modules_api::StateValue<String>, accessor: &mut impl sov_modules_api::StateAccessor) {
///     if let Some(original) = value.get(accessor) {
///         println!("original: {}", original);
///     }
///     value.delete(accessor);
/// }
///
///
/// ```
pub trait StateAccessor: StateReaderAndWriter {}

impl<S: Spec> StateAccessor for WorkingSet<S> {}

impl<S: Spec> StateAccessor for StateCheckpoint<S> {}

impl<S, V, Codec> StateValueAccessor<V, Codec, S> for StateValue<V, Codec>
where
    Codec: StateCodec,
    Codec::ValueCodec: StateValueCodec<V>,
    S: StateAccessor,
{
    fn namespace(&self) -> Namespace {
        Self::NAMESPACE
    }

    fn prefix(&self) -> &Prefix {
        &self.prefix
    }

    fn codec(&self) -> &Codec {
        &self.codec
    }
}

impl<S, K, V, Codec> StateMapAccessor<K, V, Codec, S> for StateMap<K, V, Codec>
where
    S: StateAccessor,
    Codec: StateCodec,
    Codec::KeyCodec: StateKeyCodec<K>,
    Codec::ValueCodec: StateValueCodec<V>,
{
    fn namespace(&self) -> Namespace {
        Self::NAMESPACE
    }

    /// Returns a reference to the codec used by this [`StateMap`].
    fn codec(&self) -> &Codec {
        &self.codec
    }

    /// Returns the prefix used when this [`StateMap`] was created.
    fn prefix(&self) -> &Prefix {
        &self.prefix
    }
}

impl<S, V, Codec> StateVecPrivateAccessor<V, Codec, S> for StateVec<V, Codec>
where
    S: StateAccessor,
    Codec: StateCodec + Clone,
    Codec::ValueCodec: StateValueCodec<V> + StateValueCodec<usize>,
    Codec::KeyCodec: StateKeyCodec<usize>,
{
    type ElemsMap = StateMap<usize, V, Codec>;

    type LenValue = StateValue<usize, Codec>;

    fn set_len(&self, length: usize, state_checkpoint: &mut S) {
        self.len_value.set(&length, state_checkpoint);
    }

    fn elems(&self) -> &Self::ElemsMap {
        &self.elems
    }

    fn len_value(&self) -> &Self::LenValue {
        &self.len_value
    }
}

impl<S, V, Codec> StateVecAccessor<V, Codec, S> for StateVec<V, Codec>
where
    Codec: StateCodec + Clone,
    Codec::ValueCodec: StateValueCodec<V> + StateValueCodec<usize>,
    Codec::KeyCodec: StateKeyCodec<usize>,
    S: StateAccessor,
{
    /// Returns the prefix used when this [`StateVec`] was created.
    fn prefix(&self) -> &Prefix {
        &self.prefix
    }
}
