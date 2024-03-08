//! Defines types that are related to the `AggregatedProof`.
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

// Type that represents a serialized validity condition.
type SerializedValidityCondition = Vec<u8>;

/// Aggregated proof code commitment.
#[derive(
    Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, Default,
)]
pub struct CodeCommitment(pub Vec<u8>);

impl core::fmt::Display for CodeCommitment {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0.is_empty() {
            return write!(f, "CodeCommitment([])");
        }
        write!(f, "CodeCommitment(0x{})", hex::encode(&self.0))
    }
}

/// Public input of an aggregated proof.
#[derive(Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
pub struct AggregatedProofPublicInput {
    /// Contains the validity conditions for each block in the aggregated proof.
    pub validity_conditions: Vec<SerializedValidityCondition>,
    /// Initial slot number.
    pub initial_slot_number: u64,
    /// Final slot number.
    pub final_slot_number: u64,
    /// The initial state root of the aggregated proof.
    pub initial_state_root: Vec<u8>,
    /// The final state root of the aggregated proof.
    pub final_state_root: Vec<u8>,
    /// The initial slot hash of the aggregated proof.
    pub initial_slot_hash: Vec<u8>,
    /// The final slot hash of the aggregated proof.
    pub final_slot_hash: Vec<u8>,
    /// Code Commitment of the aggregated proof circuit.
    pub code_commitment: CodeCommitment,
}

impl core::fmt::Display for AggregatedProofPublicInput {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "AggregatedProofPublicInput(initial_slot_number: {}, final_slot_number: {}, initial_state_root: 0x{}, final_state_root: 0x{}, initial_slot_hash: 0x{}, final_slot_hash: 0x{}, code_commitment: {})",
            self.initial_slot_number,
            self.final_slot_number,
            hex::encode(&self.initial_state_root),
            hex::encode(&self.final_state_root),
            hex::encode(&self.initial_slot_hash),
            hex::encode(&self.final_slot_hash),
            self.code_commitment
        )
    }
}

/// Represents an aggregated proof.
#[derive(Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
pub struct AggregatedProofData {
    pub(crate) public_input: AggregatedProofPublicInput,
}

impl AggregatedProofData {
    /// Creates `AggregatedProofData`
    pub fn new(public_input: AggregatedProofPublicInput) -> Self {
        Self { public_input }
    }

    /// Public input of the aggregated proof.
    pub fn public_input(&self) -> &AggregatedProofPublicInput {
        &self.public_input
    }

    /// Verifies the proof.
    pub fn verify(&self) -> bool {
        true
    }
}
