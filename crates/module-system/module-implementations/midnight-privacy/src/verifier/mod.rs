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

#[cfg(feature = "verify-risc0")]
pub mod risc0 {
    use super::*;
    use sov_risc0_adapter::{Risc0MethodId, Risc0Verifier};
    use sov_rollup_interface::zk::{CodeCommitment, ZkVerifier};

    #[derive(Default, Clone)]
    pub struct Risc0Spend;

    impl SpendVerifier for Risc0Spend {
        fn verify(&self, proof_bytes: &[u8], expect_vk_hash: Hash32) -> anyhow::Result<SpendPublic> {
            // Treat the configured vk_hash as the RISC0 method ID/code commitment
            let method_id = Risc0MethodId::decode(&expect_vk_hash)
                .map_err(|e| anyhow::anyhow!("invalid method_id bytes: {e}"))?;
            let public: SpendPublic = Risc0Verifier::verify(proof_bytes, &method_id)?;
            Ok(public)
        }
    }
}
