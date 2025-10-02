use sov_modules_api::macros::serialize;

/// Events emitted by the ZkPoc module
#[derive(Debug, PartialEq, Clone, schemars::JsonSchema)]
#[serialize(Borsh, Serde)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    /// Emitted when the value is set successfully
    Set { value: u64 },
}

