use std::marker::PhantomData;

use sov_modules_core::Prefix;
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
pub struct StateValue<V, Codec = BorshCodec> {
    _phantom: PhantomData<V>,
    pub(crate) codec: Codec,
    pub(crate) prefix: Prefix,
}

impl<V> StateValue<V> {
    /// Crates a new [`StateValue`] with the given prefix and the default
    /// [`sov_modules_core::StateValueCodec`] (i.e. [`BorshCodec`]).
    pub fn new(prefix: Prefix) -> Self {
        Self::with_codec(prefix, BorshCodec)
    }
}

impl<V, Codec> StateValue<V, Codec> {
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
