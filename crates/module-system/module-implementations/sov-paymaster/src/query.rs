use sov_modules_api::Spec;

use super::Paymaster;

#[derive(serde::Serialize, serde::Deserialize, Debug, Eq, PartialEq)]
pub struct Response {
    pub value: Option<u32>,
}

impl<S: Spec> Paymaster<S> {}
