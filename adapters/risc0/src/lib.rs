#![deny(missing_docs)]
//! # RISC0 Adapter
//!
//! This crate contains an adapter allowing the Risc0 to be used as a proof system for
//! Sovereign SDK rollups.
use crypto::{Risc0PublicKey, Risc0Signature};
use risc0_zkvm::sha::Digest;
#[cfg(not(target_os = "zkvm"))]
use risc0_zkvm::Journal;
#[cfg(not(target_os = "zkvm"))]
use risc0_zkvm::Receipt;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
#[cfg(not(target_os = "zkvm"))]
use sov_rollup_interface::zk::Proof;
use sov_rollup_interface::zk::{CryptoSpec, Matches, Zkvm};

pub mod crypto;
pub mod guest;
#[cfg(feature = "native")]
pub mod host;

#[cfg(feature = "bench")]
pub mod metrics;

/// Uniquely identifies a Risc0 binary. Roughly equivalent to
/// the hash of the ELF file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Risc0MethodId([u32; 8]);

impl Matches<Self> for Risc0MethodId {
    fn matches(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Matches<Digest> for Risc0MethodId {
    fn matches(&self, other: &Digest) -> bool {
        self.0 == other.as_words()
    }
}

impl Matches<[u32; 8]> for Risc0MethodId {
    fn matches(&self, other: &[u32; 8]) -> bool {
        &self.0 == other
    }
}

/// The cryptographic primitives provided by the Risc0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Copy)]
pub struct Risc0CryptoSpec;

impl CryptoSpec for Risc0CryptoSpec {
    #[cfg(feature = "native")]
    type PrivateKey = crate::crypto::private_key::Risc0PrivateKey;
    type PublicKey = Risc0PublicKey;
    type Hasher = sha2::Sha256;
    type Signature = Risc0Signature;
}

/// A verifier for Risc0 proofs.
pub struct Risc0Verifier;

#[cfg(not(target_os = "zkvm"))]
impl Zkvm for Risc0Verifier {
    type CodeCommitment = Risc0MethodId;
    type CryptoSpec = Risc0CryptoSpec;
    type Error = anyhow::Error;

    fn verify<T: DeserializeOwned>(
        serialized_proof: &[u8],
        code_commitment: &Self::CodeCommitment,
    ) -> Result<T, Self::Error> {
        let proof: Proof<Receipt, Option<Journal>> = bincode::deserialize(serialized_proof)?;
        match proof {
            Proof::PublicInput(_) => anyhow::bail!("Risc0Verifier supports only full proofs"),
            Proof::Full(receipt) => {
                receipt.verify(code_commitment.0)?;
                Ok(bincode::deserialize(&receipt.journal.bytes)?)
            }
        }
    }
}

#[cfg(target_os = "zkvm")]
impl Zkvm for Risc0Verifier {
    type CodeCommitment = Risc0MethodId;

    type CryptoSpec = Risc0CryptoSpec;

    type Error = anyhow::Error;

    fn verify<T: DeserializeOwned>(
        _serialized_proof: &[u8],
        _code_commitment: &Self::CodeCommitment,
    ) -> Result<T, Self::Error> {
        // Implement this method once risc0 supports recursion: issue #633
        todo!("Implement once risc0 supports recursion: https://github.com/Sovereign-Labs/sovereign-sdk/issues/633")
    }
}
