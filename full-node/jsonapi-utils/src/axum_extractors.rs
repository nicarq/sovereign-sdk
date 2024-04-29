//! Custom [`axum`] extractors for opinionated JSON APIs.
//!
//! See also: <https://docs.rs/axum/latest/axum/extract/index.html#customizing-extractor-responses>.

#![deny(missing_docs)]

use std::fmt::Debug;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{StatusCode, Uri};
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::types::{ApiResponse, ErrorObject, ResponseObject};
use crate::json_obj;

/// The "error" ("rejection" in [`axum`] terminology) type for [`ValidatedQuery`].
pub type ValidatedQueryRejection = ApiResponse;

/// An alternative to the built-in Axum extractor [`axum::extract::Query`],
/// which handles properly formatted JSON errors upon deserialization failure
/// according to our intended API. It also performs validation as defined by
/// [`QueryStringValidation`].
///
/// See:
/// - <https://github.com/tokio-rs/axum/issues/1116>
/// - <https://github.com/tokio-rs/axum/blob/main/examples/customize-extractor-error/src/derive_from_request.rs>
/// - <https://docs.rs/axum/latest/axum/extract/index.html#customizing-extractor-responses>
#[derive(Debug, derive_more::Deref)]
pub struct ValidatedQuery<T>(pub T);

impl<T> ValidatedQuery<T>
where
    T: DeserializeOwned + QueryStringValidation,
{
    /// Attempts to deserialize and then validate the query string from the
    /// given [`Uri`].
    pub fn try_from_uri(uri: &Uri) -> Result<Self, ValidatedQueryRejection> {
        let query_string = uri.query().unwrap_or_default();

        match serde_urlencoded::from_str::<T>(query_string) {
            Ok(query) => {
                if let Err(err) = query.validate() {
                    let response_obj = ResponseObject {
                        errors: vec![ErrorObject {
                            status: StatusCode::BAD_REQUEST.as_u16() as _,
                            title: "Invalid query string".to_string(),
                            details: json_obj!({
                                "message": err.to_string(),
                            }),
                        }],
                        ..Default::default()
                    };
                    Err((StatusCode::BAD_REQUEST, axum::Json(response_obj)))
                } else {
                    Ok(ValidatedQuery(query))
                }
            }
            Err(err) => {
                let response_obj = ResponseObject {
                    errors: vec![ErrorObject {
                        status: StatusCode::BAD_REQUEST.as_u16() as _,
                        title: "Invalid query string".to_string(),
                        details: json_obj!({
                            "message": err.to_string(),
                        }),
                    }],
                    ..Default::default()
                };

                Err((StatusCode::BAD_REQUEST, axum::Json(response_obj)))
            }
        }
    }
}

#[axum::async_trait]
impl<S, T> FromRequestParts<S> for ValidatedQuery<T>
where
    T: DeserializeOwned + QueryStringValidation,
{
    type Rejection = ValidatedQueryRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Self::try_from_uri(&parts.uri)
    }
}

/// Defines custom query string validation rules that are run during
/// [`ValidatedQuery`] extraction.
pub trait QueryStringValidation {
    /// Performs custom validation on the query string.
    fn validate(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// We never serialize tuples as query strings in production code, only custom
/// `struct`s.
#[cfg(test)]
impl<T> QueryStringValidation for &[(&str, T)] where T: serde::Serialize {}

/// An alternative to the built-in Axum extractor [`axum::extract::Path`], which
/// handles errors gracefully and returns error responses in a `JSON:API`-like
/// format.
#[derive(Debug, derive_more::Deref)]
pub struct PathWithErrorHandling<T>(pub T);

#[axum::async_trait]
impl<S, T> FromRequestParts<S> for PathWithErrorHandling<T>
where
    axum::extract::Path<T>: FromRequestParts<S>,
    <axum::extract::Path<T> as FromRequestParts<S>>::Rejection: ToString + Debug,
    S: Send + Sync,
{
    type Rejection = (StatusCode, axum::Json<Value>);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Path::from_request_parts(parts, state).await {
            Ok(query) => Ok(PathWithErrorHandling(query.0)),
            Err(err) => {
                let response_obj = ResponseObject {
                    errors: vec![ErrorObject {
                        status: StatusCode::BAD_REQUEST.as_u16() as _,
                        title: "Failed to deserialize path string parameter(s)".to_string(),
                        details: json_obj!({
                            "message": err.to_string(),
                        }),
                    }],
                    ..Default::default()
                };

                Err((
                    StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::to_value(response_obj).unwrap()),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // TODO: add tests for `PathWithErrorHandling`.

    mod validated_query {
        use super::*;
        use crate::test_utils::uri_with_query_params;

        #[derive(Debug, serde::Deserialize)]
        struct TestQuery {
            integer: u8,
        }

        impl QueryStringValidation for TestQuery {
            fn validate(&self) -> anyhow::Result<()> {
                if self.integer == 0 {
                    Err(anyhow::anyhow!("Integer must be > 0"))
                } else {
                    Ok(())
                }
            }
        }

        #[test]
        fn query_serde_error() {
            let uri = uri_with_query_params([("integer", "foo")]);
            let result = ValidatedQuery::<TestQuery>::try_from_uri(&uri);
            let err = result.unwrap_err();

            assert_eq!(err.0, StatusCode::BAD_REQUEST);
            assert_eq!(
                err.1 .0,
                ResponseObject {
                    errors: vec![ErrorObject {
                        status: StatusCode::BAD_REQUEST.as_u16() as _,
                        title: "Invalid query string".to_string(),
                        details: json_obj!({
                            "message": "invalid digit found in string"
                        }),
                    }],
                    ..Default::default()
                }
            );
        }

        #[test]
        fn query_validation_error() {
            let uri = uri_with_query_params([("integer", 0)]);
            let result = ValidatedQuery::<TestQuery>::try_from_uri(&uri);
            let err = result.unwrap_err();

            assert_eq!(err.0, StatusCode::BAD_REQUEST);
            assert_eq!(
                err.1 .0,
                ResponseObject {
                    errors: vec![ErrorObject {
                        status: StatusCode::BAD_REQUEST.as_u16() as _,
                        title: "Invalid query string".to_string(),
                        details: json_obj!({
                            "message": "Integer must be > 0"
                        }),
                    }],
                    ..Default::default()
                }
            );
        }

        #[test]
        fn query_ok() {
            let uri = uri_with_query_params([("integer", 42)]);
            let result = ValidatedQuery::<TestQuery>::try_from_uri(&uri);
            assert!(result.is_ok());
        }
    }
}
