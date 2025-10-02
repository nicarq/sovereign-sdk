use serde::{Deserialize, Serialize};

use crate::state::{Commitment, Hash32, Nullifier};

/// Public inputs the circuit commits to
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpendPublic {
    pub anchor_root: Hash32,
    pub nullifiers: Vec<Nullifier>,
    pub commitments: Vec<Commitment>,
    pub fee: u128,
    pub chain_id: Hash32,
    pub module_id: Hash32,
    pub vk_hash: Hash32,
    pub audit_commitment: Hash32,
}

pub trait SpendVerifier {
    fn verify(&self, proof_bytes: &[u8], expect_vk_hash: Hash32) -> anyhow::Result<SpendPublic>;
}

pub mod mock {
    use super::*;

    #[derive(Default, Clone)]
    pub struct AcceptAll;

    impl SpendVerifier for AcceptAll {
        fn verify(&self, proof_bytes: &[u8], _expect_vk_hash: Hash32) -> anyhow::Result<SpendPublic> {
            // Expect the caller to pass bincode-serialized SpendPublic in tests
            let public: SpendPublic = bincode::deserialize(proof_bytes)?;
            Ok(public)
        }
    }
}

