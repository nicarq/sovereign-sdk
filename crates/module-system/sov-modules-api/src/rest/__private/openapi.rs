use std::collections::HashMap;
use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::json;
use sov_state::{CompileTimeNamespace, StateCodec, StateItemCodec};
pub use utoipa::openapi::OpenApi;
use utoipa::openapi::PathItem;

use super::StateItemInfo;
use crate::containers::map::NamespacedStateMap;
use crate::containers::value::NamespacedStateValue;
use crate::containers::vec::NamespacedStateVec;

const OPENAPI_TEMPLATE: &str = include_str!("openapi-templates/runtime-base.yaml");

type OpenApiPaths = Vec<(String, serde_json::Value)>;

/// This should ideally be a reusable parameter defined in `#/parameters`, but
/// [`utoipa::openapi::path::Parameter`] doesn't support `$ref` at the time of
/// writing.
fn rollup_height_param() -> utoipa::openapi::path::Parameter {
    serde_json::from_value(json!({
        "name": "rollup_height",
        "in": "query",
        "description": "The height of the rollup to query. If not provided, the rollup head is used.",
        "required": false,
        "schema": {
            "type": "integer",
            "minimum": 0,
        }
    }))
    .unwrap()
}

/// The OpenAPI paths specification for
/// [`StateValue`](crate::containers::StateValue).
pub fn state_value_paths() -> OpenApiPaths {
    vec![(
        "".to_string(),
        json!({
            "get": {
                "summary": "Get the value of a `StateValue`.",
                "parameters": [rollup_height_param()],
                "responses": {
                    "200": {
                        "$ref": "#/components/responses/StateValueResponse"
                    }
                }
            }
        }),
    )]
}

/// The OpenAPI paths specification for
/// [`StateMap`](crate::containers::StateMap).
pub fn state_map_paths() -> OpenApiPaths {
    vec![(
        "/items/{key}".to_string(),
        json!({
            "get": {
                "summary": "Get the value of a `StateMap` element.",
                "parameters": [
                    {
                        "name": "key",
                        "in": "path",
                        "required": true,
                        "schema": {
                            "type": "string",
                        }
                    },
                    rollup_height_param(),
                ],
                "responses": {
                    "200": {
                        "$ref": "#/components/responses/StateMapElementResponse"
                    },
                    "400": {
                        "$ref": "#/components/responses/BadRequestResponse"
                    }
                }
            }
        }),
    )]
}

/// The OpenAPI paths specification for
/// [`StateVec`](crate::containers::StateVec).
pub fn state_vec_paths() -> OpenApiPaths {
    vec![
        (
            "".to_string(),
            json!({
                "get": {
                    "summary": "Get general information about a `StateVec`, including its length.",
                    "parameters": [rollup_height_param()],
                    "responses": {
                        "200": {
                            "$ref": "#/components/responses/StateVecResponse"
                        },
                        "400": {
                            "$ref": "#/components/responses/BadRequestResponse"
                        }
                    }
                }
            }),
        ),
        (
            "/items/{index}".to_string(),
            json!({
                "get": {
                    "summary": "Get the value of a `StateVec` element.",
                    "parameters": [
                        {
                            "name": "index",
                            "in": "path",
                            "required": true,
                            "schema": {
                                "type": "integer",
                                "minimum": 0,
                                "maximum": null
                            }
                        },
                        rollup_height_param(),
                    ],
                    "responses": {
                        "200": {
                            "$ref": "#/components/responses/StateVecElementResponse"
                        }
                    }
                }
            }),
        ),
    ]
}

pub fn runtime_spec(module_specs: HashMap<String, OpenApi>) -> OpenApi {
    let mut runtime_spec: OpenApi = serde_yaml::from_str(OPENAPI_TEMPLATE).unwrap();

    for (module_name, mut module_spec) in module_specs {
        let old_paths = std::mem::take(&mut module_spec.paths);
        // TODO: Extensions
        for (path, path_item) in old_paths.paths {
            let runtime_path = format!("/modules/{}{}", module_name, path);
            module_spec.paths.paths.insert(runtime_path, path_item);
        }
        runtime_spec.merge(module_spec);
    }

    runtime_spec
}

#[derive(derivative::Derivative)]
#[derivative(Clone(bound = ""))]
pub struct StateItemOpenApiSpecImpl<T> {
    pub state_item_info: StateItemInfo,
    pub phantom: PhantomData<T>,
}

pub trait StateItemOpenApiSpec {
    fn state_item_open_api(&self) -> OpenApi;
}

impl<T> StateItemOpenApiSpec for &T {
    fn state_item_open_api(&self) -> OpenApi {
        OpenApi::default()
    }
}

impl<N, T, Codec> StateItemOpenApiSpec
    for StateItemOpenApiSpecImpl<NamespacedStateValue<N, T, Codec>>
where
    N: CompileTimeNamespace,
    T: Serialize + Send + Sync + 'static,
    Codec: StateCodec,
    Codec::ValueCodec: StateItemCodec<T>,
{
    fn state_item_open_api(&self) -> OpenApi {
        let paths = state_value_paths();
        spec_from_json_paths(paths)
    }
}

impl<N, T, Codec> StateItemOpenApiSpec for StateItemOpenApiSpecImpl<NamespacedStateVec<N, T, Codec>>
where
    N: CompileTimeNamespace,
    T: Serialize + Clone + Send + Sync + 'static,
    Codec: StateCodec,
    Codec::KeyCodec: StateItemCodec<usize>,
    Codec::ValueCodec: StateItemCodec<T> + StateItemCodec<usize>,
{
    fn state_item_open_api(&self) -> OpenApi {
        let paths = state_vec_paths();
        spec_from_json_paths(paths)
    }
}

impl<N, K, V, Codec> StateItemOpenApiSpec
    for StateItemOpenApiSpecImpl<NamespacedStateMap<N, K, V, Codec>>
where
    N: CompileTimeNamespace,
    K: Serialize + DeserializeOwned + Clone + Send + Sync + 'static,
    V: Serialize + Clone + Send + Sync + 'static,
    Codec: StateCodec,
    Codec::KeyCodec: StateItemCodec<K>,
    Codec::ValueCodec: StateItemCodec<V>,
{
    fn state_item_open_api(&self) -> OpenApi {
        let paths = state_map_paths();
        spec_from_json_paths(paths)
    }
}

fn spec_from_json_paths(paths: OpenApiPaths) -> OpenApi {
    let mut item_spec = OpenApi::default();
    for (field_name, raw_path) in paths {
        let path_item: PathItem = serde_json::from_value(raw_path).unwrap();
        item_spec.paths.paths.insert(field_name, path_item);
    }
    item_spec
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_template_is_valid() {
        let _: OpenApi = serde_yaml::from_str(OPENAPI_TEMPLATE).unwrap();
    }
}
