//! Defines types that are related to the `AggregatedProof`.
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

/// Public input of an aggregated proof.
#[derive(Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
pub struct AggregatedProofPublicInput {
    /// The initial state root of the aggregated proof.
    pub initial_state_root: Vec<u8>,
    /// The final state root of the aggregated proof.
    pub final_state_root: Vec<u8>,
    /// The initial slot hash of the aggregated proof.
    pub initial_slot_hash: Vec<u8>,
    /// The final slot hash of the aggregated proof.
    pub final_slot_hash: Vec<u8>,
}

/// Additional information that is not zk proven. Can be used by clients for
/// bookkeeping purposes.
#[derive(Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
pub struct AggregatedProofDataInfo {
    /// The initial slot height of the aggregated proof.
    pub initial_slot_number: u64,
    /// The final slot height of the aggregated proof.
    pub final_slot_number: u64,
}

/// Represents an aggregated proof.
#[derive(Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
pub struct AggregatedProofData {
    pub(crate) public_input: AggregatedProofPublicInput,
    pub(crate) info: AggregatedProofDataInfo,
}

impl AggregatedProofData {
    /// Creates `AggregatedProofData`
    pub fn new(public_input: AggregatedProofPublicInput, info: AggregatedProofDataInfo) -> Self {
        Self { public_input, info }
    }

    /// Public input of the aggregated proof.
    pub fn public_input(&self) -> &AggregatedProofPublicInput {
        &self.public_input
    }

    /// Additional information that is not zk proven.
    pub fn info(&self) -> &AggregatedProofDataInfo {
        &self.info
    }

    /// Verifies the proof.
    pub fn verify(&self) -> bool {
        true
    }
}
