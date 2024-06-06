use sov_modules_api::WorkingSet;

use super::ExampleModule;

#[derive(serde::Serialize, serde::Deserialize, Debug, Eq, PartialEq)]
pub struct Response {
    pub value: Option<u32>,
}

impl<S: sov_modules_api::Spec> ExampleModule<S> {
    /// Queries the state of the module.
    pub fn query_value(&self, state: &mut WorkingSet<S>) -> Response {
        Response {
            value: self.value.get(state),
        }
    }
}
