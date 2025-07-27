//! Module error definitions.

use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sov_rest_utils::{json_obj, JsonObject};

/// A bech32 address parse error.
#[derive(Debug, thiserror::Error)]
pub enum Bech32ParseError {
    /// Bech32 decoding error represented via [`bech32::primitives::decode::CheckedHrpstringError`].
    #[error("Bech32 error: {0}")]
    Bech32(#[from] bech32::primitives::decode::CheckedHrpstringError),
    /// The provided "Human-Readable Part" is invalid.
    #[error("Wrong HRP: {0}")]
    WrongHRP(String),
    /// The provided address length is invalid.
    #[error("Wrong address length. Expected: {0} bytes, got: {1}")]
    WrongLength(usize, usize),
}

/// test
pub trait ErrorDetail: std::fmt::Debug {
    /// test
    fn error_detail(&self) -> JsonObject;
}

impl ErrorDetail for anyhow::Error {
    fn error_detail(&self) -> JsonObject {
        json_obj!({"error": self.to_string()})
    }
}

#[derive(Clone, Debug)]
struct DeserializedError {
    data: serde_json::Value,
}

impl ErrorDetail for DeserializedError {
    fn error_detail(&self) -> JsonObject {
        match &self.data {
            serde_json::Value::Object(map) => map.clone(),
            _ => panic!("data wasn't a JSON object: {:?}", self.data),
        }
    }
}

/// test
#[derive(Clone, Debug)]
pub struct ModuleError {
    inner: Arc<dyn ErrorDetail + Send + Sync>,
}

impl ModuleError {
    /// Returns the error details JSON object.
    pub fn error_detail(&self) -> JsonObject {
        self.inner.error_detail()
    }
}

// `ModuleError` must derive Display.
// Dummy implementation to relax the trait bound on inner errors.
impl std::fmt::Display for ModuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Serialize for ModuleError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.inner.error_detail().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ModuleError {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let deserialized_error = DeserializedError { data: value };

        Ok(ModuleError {
            inner: Arc::new(deserialized_error),
        })
    }
}

impl PartialEq<ModuleError> for ModuleError {
    fn eq(&self, other: &ModuleError) -> bool {
        self.to_string() == other.to_string()
    }
}

impl Eq for ModuleError {}

impl<T: ErrorDetail + Send + Sync + 'static> From<T> for ModuleError {
    fn from(value: T) -> Self {
        ModuleError {
            inner: Arc::new(value),
        }
    }
}
