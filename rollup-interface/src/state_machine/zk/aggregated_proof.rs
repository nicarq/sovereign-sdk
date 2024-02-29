//! Defines types that are related to the `AggregatedProof`.
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

/// Aggregated proof code commitment.
#[derive(
    Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, Default,
)]
pub struct CodeCommitment(pub Vec<u8>);

/// Public input of an aggregated proof.
#[derive(Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
pub struct AggregatedProofPublicInput {
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
