#![forbid(unsafe_code)]

#[cfg(feature = "server")]
pub mod rest;
pub mod rpc;

/// A 32-byte hash [`serde`]-encoded as a hex string optionally prefixed with
/// `0x`. See [`sov_rollup_interface::rpc::utils::rpc_hex`].
#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HexHash(#[serde(with = "sov_rollup_interface::rpc::utils::rpc_hex")] pub [u8; 32]);

/// A variable length byte sequence, [`serde`]-encoded as a hex string optionally prefixed with
/// `0x`. See [`sov_rollup_interface::rpc::utils::rpc_hex`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HexBytes(#[serde(with = "sov_rollup_interface::rpc::utils::rpc_hex")] pub Vec<u8>);
