//! Configuration for [`crate::da_service::CelestiaService`]
use std::num::NonZero;

use schemars::JsonSchema;

use crate::verifier::address::CelestiaAddress;

/// Runtime configuration for the [`sov_rollup_interface::node::da::DaService`] implementation.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize, JsonSchema)]
pub struct CelestiaConfig {
    /// The JWT used to authenticate with the Celestia RPC server
    pub celestia_rpc_auth_token: String,
    /// The address of the Celestia RPC server
    #[serde(default = "default_rpc_addr")]
    pub celestia_rpc_address: String,
    /// The maximum size of a Celestia RPC response, in bytes
    #[serde(default = "default_max_response_size")]
    pub max_celestia_response_body_size: NonZero<u32>,
    /// The timeout for a Celestia RPC request, in seconds
    #[serde(default = "default_request_timeout_seconds")]
    pub celestia_rpc_timeout_seconds: NonZero<u64>,
    /// See [`sov_rollup_interface::node::da::DaService::safe_lead_time`].
    #[serde(default = "default_safe_lead_time_ms")]
    pub safe_lead_time_ms: u64,
    /// The sequencer address that will be used as the signer for the blobs.
    pub signer_address: CelestiaAddress,
}

fn default_safe_lead_time_ms() -> u64 {
    500
}

fn default_rpc_addr() -> String {
    "http://localhost:11111/".into()
}

fn default_max_response_size() -> NonZero<u32> {
    // 100 MiB
    NonZero::new(1024 * 1024 * 100).unwrap()
}

pub(crate) fn default_request_timeout_seconds() -> NonZero<u64> {
    NonZero::new(60).unwrap()
}
