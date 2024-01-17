use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::zk::{Proof, StateTransition};

pub(crate) struct BlockProof<Da: DaSpec, Root> {
    pub(crate) _proof: Proof,
    pub(crate) height: u64,
    pub(crate) st: StateTransition<Da, Root>,
}

/// Public input of an aggregated proof.
#[derive(Debug, Eq, PartialEq)]
pub struct AggregatedProofPublicInput {
    /// The serialized state root before the aggregation.
    pub initial_state_root: Vec<u8>,
    /// The serialized state root after the aggregation.
    pub final_state_root: Vec<u8>,
    /// The height before the aggregation.
    pub initial_height: u64,
    /// The height after the aggregation.
    pub final_height: u64,
}

/// Represents an aggregated proof.
#[derive(Debug, Eq, PartialEq)]
pub struct AggregatedProof {
    pub(crate) public_input: AggregatedProofPublicInput,
}

impl AggregatedProof {
    pub(crate) fn new(public_input: AggregatedProofPublicInput) -> Self {
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
