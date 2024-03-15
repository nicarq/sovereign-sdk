#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

use std::collections::VecDeque;
mod notifier;
use borsh::{BorshDeserialize, BorshSerialize};
use notifier::NotificationManager;
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

/// A mock implementing the zkVM trait.
#[derive(Clone)]
pub struct MockZkvm {
    notification_manager: NotificationManager,
    committed_data: VecDeque<Vec<u8>>,
}

impl MockZkvm {
    /// Creates a new MockZkvm
    pub fn new() -> Self {
        Self {
            notification_manager: Default::default(),
            committed_data: Default::default(),
        }
    }

    /// Simulates zk proof generation.
    pub fn make_proof(&self) {
        // We notify the worker thread.
        self.notification_manager.notify();
    }

    /// Create a proof for MockZkvm
    pub fn create_serialized_proof<T: Serialize>(is_valid: bool, transition: T) -> Vec<u8> {
        let data = bincode::serialize(&transition).unwrap();
        bincode::serialize(&Proof::<(), Inner>::PublicInput(Inner {
            is_valid,
            pub_input: data,
        }))
        .unwrap()
    }
}

impl Default for MockZkvm {
    fn default() -> Self {
        Self::new()
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
    pub_input: Vec<u8>,
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
            Proof::PublicInput(Inner {
                is_valid,
                pub_input: input,
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

impl sov_rollup_interface::zk::ZkvmHost for MockZkvm {
    type Guest = MockZkGuest;

    fn add_hint<T: Serialize>(&mut self, item: T) {
        let data = bincode::serialize(&item).unwrap();
        self.committed_data.push_back(data);
    }

    fn simulate_with_hints(&mut self) -> Self::Guest {
        MockZkGuest {}
    }

    fn run(&mut self, _with_proof: bool) -> Result<Vec<u8>, anyhow::Error> {
        self.notification_manager.wait();
        let data = self.committed_data.pop_front().unwrap_or_default();
        Ok(bincode::serialize(&sov_rollup_interface::zk::Proof::<
            Empty,
            _,
        >::PublicInput(Inner {
            is_valid: true,
            pub_input: data,
        }))?)
    }
}

/// A mock implementing the Guest.
pub struct MockZkGuest {}

impl sov_rollup_interface::zk::ZkvmGuest for MockZkGuest {
    type Verifier = MockZkVerifier;
    fn read_from_host<T: serde::de::DeserializeOwned>(&self) -> T {
        unimplemented!()
    }

    fn commit<T: Serialize>(&self, _item: &T) {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use sov_rollup_interface::zk::{Zkvm, ZkvmHost};

    use super::*;

    #[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
    struct TestPublicInput {
        hint: String,
    }

    #[test]
    fn test_mock_vm() -> Result<(), anyhow::Error> {
        let pub_input = TestPublicInput {
            hint: "Test".to_owned(),
        };

        let mut vm = MockZkvm::new();
        vm.add_hint(&pub_input);
        vm.make_proof();

        let proof = vm.run(false).unwrap();
        let verified_pub_input =
            MockZkVerifier::verify::<TestPublicInput>(&proof, &Default::default())?;

        assert_eq!(verified_pub_input, pub_input);
        Ok(())
    }

    #[test]
    fn test_proof_serialization() -> Result<(), anyhow::Error> {
        let proof = MockZkvm::create_serialized_proof(true, "Valid");
        let verified_pub_input =
            MockZkVerifier::verify::<TestPublicInput>(&proof, &Default::default());

        assert!(verified_pub_input.is_ok());

        let proof = MockZkvm::create_serialized_proof(false, "Invalid");
        let verified_pub_input =
            MockZkVerifier::verify::<TestPublicInput>(&proof, &Default::default());

        assert!(verified_pub_input.is_err());

        Ok(())
    }
}
