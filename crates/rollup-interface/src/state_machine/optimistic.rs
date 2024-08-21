//! Utilities for building an optimistic state machine
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use crate::da::DaSpec;
use crate::zk::StateTransitionPublicData;

/// A proof that the attester was bonded at the transition num `transition_num`.
/// For rollups using the `jmt`, this will be a `jmt::SparseMerkleProof`
#[derive(
    Debug,
    Clone,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
    Default,
    PartialEq,
    Eq,
    sov_wallet_format::UniversalWallet,
)]
pub struct ProofOfBond<StateProof> {
    /// The transition number for which the proof of bond applies
    pub claimed_transition_num: u64,
    /// The actual state proof that the attester was bonded
    #[sov_wallet(hidden)]
    pub proof: StateProof,
}

/// An attestation that a particular DA layer block transitioned the rollup state to some value
#[derive(
    Debug,
    Clone,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
    Default,
    PartialEq,
    Eq,
    sov_wallet_format::UniversalWallet,
)]
pub struct Attestation<SlotHash, StateProof, StateRoot> {
    /// The alleged state root before applying the contents of the da block
    pub initial_state_root: StateRoot,
    /// The hash of the block in which the transition occurred
    pub slot_hash: SlotHash,
    /// The alleged post-state root
    pub post_state_root: StateRoot,
    /// A proof that the attester was bonded at some point in time before the attestation is generated
    pub proof_of_bond: ProofOfBond<StateProof>,
}

/// The contents of a challenge to an attestation, which are contained as a public output of the proof
/// Generic over an address type and a validity condition
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
    sov_wallet_format::UniversalWallet,
)]
pub struct ChallengeContents<Address, Da: DaSpec, Root> {
    /// The rollup address of the originator of this challenge
    pub challenger_address: Address,
    /// The state transition that was proven
    #[borsh(bound(
        serialize = "Address: borsh::ser::BorshSerialize, Root: borsh::ser::BorshSerialize, Da::SlotHash: borsh::ser::BorshSerialize",
        deserialize = "Address: borsh::ser::BorshSerialize, Root: borsh::de::BorshDeserialize, Da::SlotHash: borsh::de::BorshDeserialize"
    ))]
    pub state_transition: StateTransitionPublicData<Address, Da, Root>,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BorshSerialize,
    Serialize,
    Deserialize,
    sov_wallet_format::UniversalWallet,
)]
/// This struct contains the challenge as a raw blob
pub struct Challenge<'a>(&'a [u8]);

/// Represents a serialized attestation.
#[derive(Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
pub struct SerializedAttestation {
    /// Serialized attestation.
    pub raw_attestation: Vec<u8>,
}

/// Represents a serialized challenge.
#[derive(Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
pub struct SerializedChallenge {
    /// Serialized challenge.
    pub raw_challenge: Vec<u8>,
}

impl SerializedAttestation {
    /// Serializes an attestation.
    pub fn from_attestation<
        SlotHash: BorshSerialize,
        StateProof: BorshSerialize,
        StateRoot: BorshSerialize,
    >(
        attestation: &Attestation<SlotHash, StateProof, StateRoot>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            raw_attestation: borsh::to_vec(attestation)?,
        })
    }

    /// Deserializes an attestation.
    pub fn to_attestation<
        SlotHash: BorshDeserialize,
        StateProof: BorshDeserialize,
        StateRoot: BorshDeserialize,
    >(
        &self,
    ) -> anyhow::Result<Attestation<SlotHash, StateProof, StateRoot>> {
        Ok(borsh::from_slice(&self.raw_attestation)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attestation_serialization() {
        let attestation = Attestation {
            initial_state_root: [3; 32],
            slot_hash: [10; 32],
            post_state_root: [22; 32],
            proof_of_bond: ProofOfBond {
                claimed_transition_num: 1,
                proof: (),
            },
        };

        let serialized_attestation = SerializedAttestation::from_attestation(&attestation).unwrap();
        let deserialized_attestation = serialized_attestation.to_attestation().unwrap();

        assert_eq!(attestation, deserialized_attestation);
    }
}
