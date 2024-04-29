#![forbid(unsafe_code)]

pub mod jsonapi;
pub mod rpc;

/// A 32-byte hash [`serde`]-encoded as a hex string optionally prefixed with
/// `0x`. See [`sov_rollup_interface::rpc::utils::rpc_hex`].
#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HexHash(#[serde(with = "sov_rollup_interface::rpc::utils::rpc_hex")] pub [u8; 32]);
