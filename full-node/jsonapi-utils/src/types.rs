//! Core type definitions.

/// Top-level response object to be used for all responses.
#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseObject {
    /// Core response data. It can be `null`, an array, or an object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<ResponseObjectData>,
    /// A list of errors that occurred during the request. If the list is empty,
    /// the request was successful.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ErrorObject>,
    /// Metadata about the response, if present or needed (e.g. remaining
    /// requests available in the current rate limit window). This will be empty
    /// in most cases.
    pub meta: JsonObject,
}

/// The response object's data (see [`ResponseObject`]).
#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum ResponseObjectData {
    /// The data is an object.
    Single(JsonObject),
    /// The data is an array of objects.
    Many(Vec<JsonObject>),
}

impl From<JsonObject> for ResponseObjectData {
    fn from(value: JsonObject) -> Self {
        Self::Single(value)
    }
}

impl TryFrom<serde_json::Value> for ResponseObjectData {
    type Error = anyhow::Error;

    fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
        match value {
            serde_json::Value::Object(map) => Ok(Self::Single(map)),
            _ => Err(anyhow::anyhow!("Invalid response object")),
        }
    }
}

/// A JSON object.
pub type JsonObject = serde_json::Map<String, serde_json::Value>;

/// Inspired from <https://jsonapi.org/format/#error-objects>.
#[derive(Debug, serde::Serialize)]
pub struct ErrorObject {
    /// HTTP status code that best describes the error.
    pub status: u16,
    /// A short, human-readable description of the error.
    pub title: String,
    /// Structured details about the error, if available.
    pub details: JsonObject,
}
