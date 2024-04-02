use serde_json;
use sov_modules_core::{StateItemDecoder, StateItemEncoder};

use super::StateCodec;

/// A [`StateCodec`] that uses [`serde_json`] for all keys and values.
#[derive(Debug, Default, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub struct JsonCodec;

impl<V> StateItemEncoder<V> for JsonCodec
where
    V: serde::Serialize,
{
    fn encode(&self, value: &V) -> Vec<u8> {
        serde_json::to_vec(value).expect("Failed to serialize value")
    }
}

impl<V> StateItemDecoder<V> for JsonCodec
where
    V: for<'a> serde::Deserialize<'a>,
{
    type Error = serde_json::Error;

    fn try_decode(&self, bytes: &[u8]) -> Result<V, Self::Error> {
        serde_json::from_slice(bytes)
    }
}

impl StateCodec for JsonCodec {
    type KeyCodec = Self;
    type ValueCodec = Self;

    fn key_codec(&self) -> &Self::KeyCodec {
        self
    }

    fn value_codec(&self) -> &Self::ValueCodec {
        self
    }
}
