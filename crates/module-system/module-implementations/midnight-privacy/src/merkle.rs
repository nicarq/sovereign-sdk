use crate::state::Hash32;

#[derive(Clone, Default, borsh::BorshDeserialize, borsh::BorshSerialize)]
pub struct OpaqueTree {
    leaves: Vec<Hash32>,
}

impl OpaqueTree {
    pub fn insert(&mut self, c: Hash32) -> anyhow::Result<()> {
        self.leaves.push(c);
        Ok(())
    }
    pub fn root(&self) -> anyhow::Result<Hash32> {
        Ok(hash_bytes(&bincode::serialize(&self.leaves).unwrap()))
    }
}

pub fn hash_bytes(data: &[u8]) -> Hash32 {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&h[..32]);
    out
}

