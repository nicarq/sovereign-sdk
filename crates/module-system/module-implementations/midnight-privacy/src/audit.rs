use serde::{Deserialize, Serialize};
use sov_modules_api::macros::UniversalWallet;

use crate::state::{Hash32, ViewerId};

/// Envelope sealed to a viewer VPK using X25519 + HKDF + XChaCha20-Poly1305
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema, borsh::BorshSerialize, borsh::BorshDeserialize, UniversalWallet)]
pub struct AuditCiphertext {
    pub viewer_id: ViewerId,
    pub epk: [u8; 32],
    pub nonce: [u8; 24],
    pub ct: Vec<u8>,
}

/// Tiny on-chain reference for an audit bundle
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema, borsh::BorshSerialize, borsh::BorshDeserialize, UniversalWallet)]
pub struct AuditIndexEntry {
    pub count: u16,
    pub audit_commitment: Hash32,
}
