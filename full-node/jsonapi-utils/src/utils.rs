//! Miscellaneous utilities.

use std::collections::BTreeSet;
use std::fmt::{Debug, Display};

use axum::body::Body;
use axum::extract::{OriginalUri, Request};
use axum::http::{HeaderName, StatusCode};
use axum::{Json, Router};
use tower_http::compression::CompressionLayer;
use tower_http::propagate_header::PropagateHeaderLayer;
use tower_http::trace::TraceLayer;
use tower_request_id::{RequestId, RequestIdLayer};
use tracing::{error, error_span};

use crate::types::{
    ApiResponse, ApiResponseResult, ErrorObject, ResponseObject, ResponseObjectData,
};

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

/// Customizes the given [`Router`] with a set of preconfigured "layers" that
/// are a good starting point for building production-ready JSON APIs.
pub fn preconfigured_router_layers<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    // Tracing span with unique ID per request:
    // <https://github.com/imbolc/tower-request-id/blob/main/examples/logging.rs>
    let trace_layer = TraceLayer::new_for_http().make_span_with(|request: &Request<Body>| {
        // We get the request id from the extensions
        let request_id = request
            .extensions()
            .get::<RequestId>()
            .map(ToString::to_string)
            .unwrap_or_else(|| "unknown".into());
        // And then we put it along with other information into the `request` span
        error_span!(
            "request",
            id = %request_id,
            method = %request.method(),
            uri = %request.uri(),
        )
    });
    router
        .layer(trace_layer)
        // This layer creates a new id for each request and puts it into the request extensions.
        // Note that it should be added after the Trace layer. (Filippo: why? I
        // don't know, I copy-pasted this.)
        .layer(RequestIdLayer)
        .layer(
            tower::ServiceBuilder::new()
                // Tracing.
                .layer(TraceLayer::new_for_http())
                // Compress responses with GZIP.
                .layer(CompressionLayer::new())
                // Propagate `X-Request-Id`s from requests to responses.
                .layer(PropagateHeaderLayer::new(HeaderName::from_static(
                    "x-request-id",
                ))),
        )
        .fallback(global_404)
}

/// A comma-separated set of strings, useful for [`serde`] (de)serialization.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
pub struct CommaSeparatedStringsSet(pub BTreeSet<String>);

impl CommaSeparatedStringsSet {
    /// Returns true iff the set is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl serde::Serialize for CommaSeparatedStringsSet {
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

impl<'a> serde::Deserialize<'a> for CommaSeparatedStringsSet {
    fn deserialize<D>(deserializer: D) -> Result<CommaSeparatedStringsSet, D::Error>
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

/// A 404 response useful as a catch-all for invalid routes.
pub async fn global_404(OriginalUri(uri): OriginalUri) -> ApiResponse {
    (
        StatusCode::NOT_FOUND,
        Json(ResponseObject {
            errors: vec![ErrorObject {
                status: StatusCode::NOT_FOUND.as_u16() as _,
                title: "Invalid URL".to_string(),
                details: json_obj!({
                    "url": uri.to_string(),
                }),
            }],
            ..Default::default()
        }),
    )
}

/// Returns a 501 error.
pub fn not_implemented_501() -> ApiResponse {
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
pub fn not_found_404(resource_name_capitalized: &str, resource_id: impl ToString) -> ApiResponse {
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

/// Returns a 500 error to be used when a database error occurred.
pub fn database_error_response_500(err: impl ToString) -> ApiResponse {
    // We don't include the database error in the response, because it may
    // contain sensitive information. But we log it.
    error!(
        error = err.to_string(),
        "Database error while serving request."
    );
    internal_server_error_response_500("Database error")
}

/// Returns a 500 internal server error.
pub fn internal_server_error_response_500(err: impl ToString) -> ApiResponse {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ResponseObject {
            errors: vec![ErrorObject {
                status: StatusCode::INTERNAL_SERVER_ERROR.as_u16() as _,
                title: "Internal server error".to_string(),
                details: json_obj!({
                    "message": err.to_string(),
                }),
            }],
            ..Default::default()
        }),
    )
}

/// Converts a [`serde`]-serializable object into a [`ResponseObjectData`] result.
pub fn serde_obj_to_data<T: serde::Serialize>(item: T) -> anyhow::Result<ResponseObjectData> {
    let json_obj = serde_json::to_value(item)?;

    match json_obj {
        serde_json::Value::Object(obj) => Ok(ResponseObjectData::Single(obj)),
        serde_json::Value::Array(obj) => {
            let objs = obj
                .into_iter()
                .map(|value| match value {
                    serde_json::Value::Object(obj) => Ok(obj),
                    // We only allow objects or arrays of objects in the
                    // response "main" data field. This is intentional, as we
                    // don't intend on serializing other kinds of responses.
                    _ => Err(anyhow::anyhow!("Invalid response object; expected object")),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ResponseObjectData::Many(objs))
        }
        _ => Err(anyhow::anyhow!(
            "Invalid response object; expected object or array",
        )),
    }
}

/// Creates a new response with *just* the data obtained from
/// [`serde_obj_to_data`]. This is often what you want unless you need a "custom"
/// response object.
pub fn serde_obj_to_response_result<T: serde::Serialize>(item: T) -> ApiResponseResult {
    let response_obj = serde_obj_to_data(item).map_err(internal_server_error_response_500)?;

    Ok((
        StatusCode::OK,
        Json(ResponseObject {
            data: Some(response_obj),
            ..Default::default()
        }),
    ))
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
            let item = CommaSeparatedStringsSet(numbers.into_iter().map(|i| i.to_string()).collect());
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
