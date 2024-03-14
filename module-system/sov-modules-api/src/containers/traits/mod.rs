mod map;
mod value;
mod vec;
pub use map::{StateMapAccessor, StateMapError};
use sov_modules_core::namespaces::{Accessory, CompileTimeNamespace, User};
use sov_modules_core::{Prefix, StateCodec, StateKeyCodec, StateReaderAndWriter, StateValueCodec};
pub use value::{StateValueAccessor, StateValueError};
#[cfg(test)]
pub use vec::tests as vec_tests;
pub use vec::{StateVecAccessor, StateVecError, StateVecPrivateAccessor};

use super::map::NamespacedStateMap;
use super::value::NamespacedStateValue;
use super::vec::NamespacedStateVec;

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
pub trait NamespacedStateAccessor<N: CompileTimeNamespace>: StateReaderAndWriter<N> {}

impl<T, N: CompileTimeNamespace> NamespacedStateAccessor<N> for T where T: StateReaderAndWriter<N> {}
pub trait StateAccessor: NamespacedStateAccessor<User> {}

impl<T> StateAccessor for T where T: NamespacedStateAccessor<User> {}

pub trait AccessoryStateAccessor: StateReaderAndWriter<Accessory> {}

impl<T> AccessoryStateAccessor for T where T: NamespacedStateAccessor<Accessory> {}

impl<S, N: CompileTimeNamespace, V, Codec> StateValueAccessor<N, V, Codec, S>
    for NamespacedStateValue<N, V, Codec>
where
    Codec: StateCodec,
    Codec::ValueCodec: StateValueCodec<V>,
    S: NamespacedStateAccessor<N>,
{
    fn prefix(&self) -> &Prefix {
        &self.prefix
    }

    fn codec(&self) -> &Codec {
        &self.codec
    }
}

impl<N, S, K, V, Codec> StateMapAccessor<N, K, V, Codec, S> for NamespacedStateMap<N, K, V, Codec>
where
    Codec: StateCodec,
    Codec::KeyCodec: StateKeyCodec<K>,
    Codec::ValueCodec: StateValueCodec<V>,
    N: CompileTimeNamespace,
    S: NamespacedStateAccessor<N>,
{
    /// Returns a reference to the codec used by this [`StateMap`].
    fn codec(&self) -> &Codec {
        &self.codec
    }

    /// Returns the prefix used when this [`StateMap`] was created.
    fn prefix(&self) -> &Prefix {
        &self.prefix
    }
}

impl<N, S, V, Codec> StateVecPrivateAccessor<N, V, Codec, S> for NamespacedStateVec<N, V, Codec>
where
    Codec: StateCodec + Clone,
    Codec::ValueCodec: StateValueCodec<V> + StateValueCodec<usize>,
    Codec::KeyCodec: StateKeyCodec<usize>,
    S: NamespacedStateAccessor<N>,
    N: CompileTimeNamespace,
{
    type ElemsMap = NamespacedStateMap<N, usize, V, Codec>;

    type LenValue = NamespacedStateValue<N, usize, Codec>;

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

impl<N, S, V, Codec> StateVecAccessor<N, V, Codec, S> for NamespacedStateVec<N, V, Codec>
where
    Codec: StateCodec + Clone,
    Codec::ValueCodec: StateValueCodec<V> + StateValueCodec<usize>,
    Codec::KeyCodec: StateKeyCodec<usize>,
    S: NamespacedStateAccessor<N>,
    N: CompileTimeNamespace,
{
    /// Returns the prefix used when this [`StateVec`] was created.
    fn prefix(&self) -> &Prefix {
        &self.prefix
    }
}
