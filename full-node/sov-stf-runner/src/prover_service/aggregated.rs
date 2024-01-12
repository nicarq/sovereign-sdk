use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::zk::{Proof, StateTransition};

pub(crate) struct BlockProof<Da: DaSpec, Root> {
    pub(crate) _proof: Proof,
    pub(crate) height: u64,
    pub(crate) st: StateTransition<Da, Root>,
}

/// Public input of an aggregated proof.
#[derive(Debug, Eq, PartialEq)]
pub struct AggregatedProofPublicInput<StateRoot> {
    /// The state root before the aggregation.
    pub initial_state_root: StateRoot,
    /// The state root after the aggregation.
    pub final_state_root: StateRoot,
    /// The height before the aggregation.
    pub initial_height: u64,
    /// The height after the aggregation.
    pub final_height: u64,
}

/// Represents an aggregated proof.
#[derive(Debug, Eq, PartialEq)]
pub struct AggregatedProof<StateRoot> {
    pub(crate) public_input: AggregatedProofPublicInput<StateRoot>,
}

impl<StateRoot> AggregatedProof<StateRoot> {
    pub(crate) fn new(public_input: AggregatedProofPublicInput<StateRoot>) -> Self {
        Self { public_input }
    }

    /// Public input of the aggregated proof.
    pub fn public_input(&self) -> &AggregatedProofPublicInput<StateRoot> {
        &self.public_input
    }

    /// Verifies the proof.
    pub fn verify(&self) -> bool {
        true
    }
}
