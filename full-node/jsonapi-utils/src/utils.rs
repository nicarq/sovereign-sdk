//! Miscellaneous utilities.

use std::fmt::{Debug, Display};

use axum::http::StatusCode;
use axum::Json;
use tracing::error;

use crate::types::{ErrorObject, ResponseObject};

/// A newtype wrapper around [`Vec<u8>`] which is serialized as a
/// 0x-prefixed hex string.
#[derive(Debug, Clone, PartialEq, Eq, derive_more::AsRef)]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
pub struct HexString(pub Vec<u8>);

impl Display for HexString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x{}", hex::encode(&self.0))
    }
}

impl serde::Serialize for HexString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'a> serde::Deserialize<'a> for HexString {
    fn deserialize<D>(deserializer: D) -> Result<HexString, D::Error>
    where
        D: serde::Deserializer<'a>,
    {
        let string = String::deserialize(deserializer)?;
        // We ignore the 0x prefix if it exists.
        let s = string.strip_prefix("0x").unwrap_or(&string);

        hex::decode(s)
            .map_err(|e| anyhow::anyhow!("failed to decode hex: {}", e))
            .map(HexString)
            .map_err(serde::de::Error::custom)
    }
}

/// A comma-separated list of strings, useful for [`serde`] (de)serialization.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
pub struct CommaSeparatedStrings(pub Vec<String>);

impl serde::Serialize for CommaSeparatedStrings {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut s = String::new();
        for (i, item) in self.0.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(item);
        }
        s.serialize(serializer)
    }
}

impl<'a> serde::Deserialize<'a> for CommaSeparatedStrings {
    fn deserialize<D>(deserializer: D) -> Result<CommaSeparatedStrings, D::Error>
    where
        D: serde::Deserializer<'a>,
    {
        let string = String::deserialize(deserializer)?;
        let strings = string
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect();

        Ok(Self(strings))
    }
}

/// Exactly like [`serde_json::Value`], but returns a JSON object instead of a
/// JSON value.
#[macro_export]
macro_rules! json_obj {
    ($($json:tt)+) => {
        match ::serde_json::json!($($json)+) {
            ::serde_json::Value::Object(obj) => obj,
            _ => panic!("json_obj! macro returned non-object value"),
        }
    };
}

/// Returns a 501 error.
pub fn not_implemented_501() -> (StatusCode, Json<ResponseObject>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(ResponseObject {
            errors: vec![ErrorObject {
                status: StatusCode::NOT_IMPLEMENTED.as_u16() as _,
                title: "Not implemented yet".to_string(),
                details: Default::default(),
            }],
            ..Default::default()
        }),
    )
}

/// Returns a 404 error when the given resource was not found.
pub fn not_found_404(
    resource_name_capitalized: &str,
    resource_id: impl ToString,
) -> (StatusCode, Json<ResponseObject>) {
    (
        StatusCode::NOT_FOUND,
        Json(ResponseObject {
            errors: vec![ErrorObject {
                status: StatusCode::NOT_FOUND.as_u16() as _,
                title: format!(
                    "{} '{}' not found",
                    resource_name_capitalized,
                    resource_id.to_string()
                ),
                details: json_obj!({
                    "id": resource_id.to_string(),
                }),
            }],
            ..Default::default()
        }),
    )
}

/// Returns a 504 error to be used when a database error occurred.
pub fn gateway_timeout_response_504(err: impl ToString) -> (StatusCode, Json<ResponseObject>) {
    // We don't include the database error in the response, because it may
    // contain sensitive information. But we log it.
    error!(
        error = err.to_string(),
        "Database error while serving request."
    );
    (
        StatusCode::GATEWAY_TIMEOUT,
        Json(ResponseObject {
            errors: vec![ErrorObject {
                status: StatusCode::GATEWAY_TIMEOUT.as_u16() as _,
                title: "Database unavailable".to_string(),
                details: Default::default(),
            }],
            ..Default::default()
        }),
    )
}

#[cfg(test)]
mod tests {
    use proptest::proptest;

    use super::*;
    use crate::test_utils::{test_serialization_roundtrip_equality_json, uri_with_query_params};

    proptest! {
        #[test]
        fn hex_string_serialization_roundtrip(item: HexString) {
            test_serialization_roundtrip_equality_json(item);
        }

        #[test]
        fn comma_separated_strings_serialization_roundtrip(numbers: Vec<i32>) {
            let item = CommaSeparatedStrings(numbers.into_iter().map(|i| i.to_string()).collect());
            test_serialization_roundtrip_equality_json(item);
        }

        // Ideally we'd also test with types other than strings. E.g. integers?
        #[test]
        fn any_query_param_can_be_serialized(key: String, value: String) {
            // As long as it doesn't crash, we're good and the test succeeds.
            uri_with_query_params([(key, value)]);
        }
    }
}
