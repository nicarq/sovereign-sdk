use std::marker::PhantomData;

use sov_modules_core::namespaces::{CompileTimeNamespace, User};
use sov_modules_core::{Namespace, Prefix};
use sov_state::codec::BorshCodec;

/// Container for a single value.
#[derive(
    Debug,
    Clone,
    PartialEq,
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct NamespacedStateValue<N, V, Codec = BorshCodec> {
    _phantom: PhantomData<(V, N)>,
    pub(crate) codec: Codec,
    pub(crate) prefix: Prefix,
}

impl<N: CompileTimeNamespace, V, Codec> NamespacedStateValue<N, V, Codec> {
    pub const NAMESPACE: Namespace = <N as CompileTimeNamespace>::NAMESPACE;
}

pub type StateValue<V, Codec = BorshCodec> = NamespacedStateValue<User, V, Codec>;

impl<N: CompileTimeNamespace, V> NamespacedStateValue<N, V> {
    /// Crates a new [`StateValue`] with the given prefix and the default
    /// [`sov_modules_core::StateValueCodec`] (i.e. [`BorshCodec`]).
    pub fn new(prefix: Prefix) -> Self {
        Self::with_codec(prefix, BorshCodec)
    }
}

impl<N, V, Codec> NamespacedStateValue<N, V, Codec> {
    /// Creates a new [`StateValue`] with the given prefix and codec.
    pub fn with_codec(prefix: Prefix, codec: Codec) -> Self {
        Self {
            _phantom: PhantomData,
            codec,
            prefix,
        }
    }

    pub fn prefix(&self) -> &Prefix {
        &self.prefix
    }
}
