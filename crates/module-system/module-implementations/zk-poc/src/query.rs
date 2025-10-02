use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::ApiStateAccessor;

use super::ZkPoc;

#[derive(serde::Serialize, serde::Deserialize, Debug, Eq, PartialEq)]
pub struct Response {
    pub value: Option<u64>,
}

impl<S: sov_modules_api::Spec> ZkPoc<S> {
    /// Queries the stored value
    pub fn query_value(&self, state: &mut ApiStateAccessor<S>) -> Response {
        Response {
            value: self.value.get(state).unwrap_infallible(),
        }
    }
}
