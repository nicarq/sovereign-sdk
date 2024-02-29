//! Defines rpc queries exposed by the module
use jsonrpsee::core::RpcResult;
use sov_modules_api::macros::rpc_gen;
use sov_modules_api::prelude::*;
use sov_modules_api::WorkingSet;

use super::ValueSetter;

/// Response returned from the valueSetter_queryValue endpoint.
#[derive(serde::Serialize, serde::Deserialize, Debug, Eq, PartialEq, Clone)]
pub struct Response {
    /// Value saved in the module's state.
    pub value: Option<u32>,
}

#[rpc_gen(client, server, namespace = "valueSetter")]
impl<S: sov_modules_api::Spec> ValueSetter<S> {
    /// Queries the state of the module.
    #[rpc_method(name = "queryValue")]
    pub fn query_value(&self, working_set: &mut WorkingSet<S>) -> RpcResult<Response> {
        Ok(Response {
            value: self.value.get(working_set),
        })
    }
}
