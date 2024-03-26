#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

#[cfg(feature = "native")]
mod notifier;
use borsh::{BorshDeserialize, BorshSerialize};
mod guest;
pub use guest::MockZkGuest;
#[cfg(feature = "native")]
mod host;
#[cfg(feature = "native")]
pub use host::MockZkvm;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
pub mod crypto;
use sov_rollup_interface::zk::{CryptoSpec, Matches, Proof};

use crate::crypto::{Ed25519PublicKey, Ed25519Signature};

/// The cryptographic primitives provided for general purpose use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Copy)]
pub struct MockZkvmCryptoSpec;

impl CryptoSpec for MockZkvmCryptoSpec {
    #[cfg(feature = "native")]
    type PrivateKey = crate::crypto::private_key::Ed25519PrivateKey;
    type PublicKey = Ed25519PublicKey;
    type Hasher = sha2::Sha256;
    type Signature = Ed25519Signature;
}

/// A mock commitment to a particular zkVM program.
#[derive(
    Debug, Clone, PartialEq, Eq, BorshDeserialize, BorshSerialize, Serialize, Deserialize, Default,
)]
pub struct MockCodeCommitment(pub [u8; 32]);

impl Matches<MockCodeCommitment> for MockCodeCommitment {
    fn matches(&self, other: &MockCodeCommitment) -> bool {
        self.0 == other.0
    }
}

/// A type that is impossible to instantiate.
#[derive(Serialize, Deserialize)]
enum Empty {}

/// A helper type capable of simulating invalid proofs.
#[derive(Serialize, Deserialize)]
struct Inner {
    /// Is proof valid.
    is_valid: bool,
    /// Public input.
    pub_data: Vec<u8>,
}

/// The verifier for mock zk proofs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockZkVerifier;

impl sov_rollup_interface::zk::Zkvm for MockZkVerifier {
    type CodeCommitment = MockCodeCommitment;

    type CryptoSpec = MockZkvmCryptoSpec;

    type Error = anyhow::Error;

    fn verify<T: DeserializeOwned>(
        serialized_proof: &[u8],
        _code_commitment: &Self::CodeCommitment,
    ) -> Result<T, Self::Error> {
        let proof: Proof<Empty, Inner> = bincode::deserialize(serialized_proof)?;
        match proof {
            Proof::PublicData(Inner {
                is_valid,
                pub_data: input,
            }) => {
                if is_valid {
                    Ok(bincode::deserialize(&input)?)
                } else {
                    anyhow::bail!("Proof is not valid")
                }
            }
            Proof::Full(_) => unimplemented!("MockZkVerifier doesn't support full zk proofs"),
        }
    }
}

#[cfg(test)]
mod tests {
    use sov_rollup_interface::zk::{Zkvm, ZkvmHost};

    use super::*;

    #[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
    struct TestPublicData {
        hint: String,
    }

    #[test]
    fn test_mock_vm() -> Result<(), anyhow::Error> {
        let pub_data = TestPublicData {
            hint: "Test".to_owned(),
        };

        let mut vm = MockZkvm::new();
        vm.add_hint(&pub_data);
        vm.make_proof();

        let proof = vm.run(false).unwrap();
        let verified_pub_data =
            MockZkVerifier::verify::<TestPublicData>(&proof, &Default::default())?;

        assert_eq!(verified_pub_data, pub_data);
        Ok(())
    }

    #[test]
    fn test_proof_serialization() -> Result<(), anyhow::Error> {
        let proof = MockZkvm::create_serialized_proof(true, "Valid");
        let verified_pub_data =
            MockZkVerifier::verify::<TestPublicData>(&proof, &Default::default());

        assert!(verified_pub_data.is_ok());

        let proof = MockZkvm::create_serialized_proof(false, "Invalid");
        let verified_pub_data =
            MockZkVerifier::verify::<TestPublicData>(&proof, &Default::default());

        assert!(verified_pub_data.is_err());

        Ok(())
    }
}
