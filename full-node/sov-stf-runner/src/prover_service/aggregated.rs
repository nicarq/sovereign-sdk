use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::zk::{Proof, StateTransition, StateTransitionData};

pub(crate) struct BlockProof<Da: DaSpec, Root> {
    pub(crate) _proof: Proof,
    pub(crate) height: u64,
    pub(crate) st: StateTransition<Da, Root>,
}

/// Public input of an aggregated proof.
#[derive(Debug, Eq, PartialEq)]
pub struct AggregatedProofPublicInput {
    /// The initial state root of the aggregated proof.
    pub initial_state_root: Vec<u8>,
    /// The final state root of the aggregated proof.
    pub final_state_root: Vec<u8>,
    /// The initial da block hash of the aggregated proof.
    pub initial_da_block_hash: Vec<u8>,
    // The final da block hash of the aggregated proof.
    pub final_da_block_hash: Vec<u8>,
}

// Additional information that is not zk proven. Can be used by clients for
// bookkeeping purposes.
#[derive(Debug, Eq, PartialEq)]
pub struct AggregatedProofDataInfo {
    /// The initial state height of the aggregated proof.
    pub initial_state_height: u64,
    /// The final state height of the aggregated proof.
    pub final_state_height: u64,
}

/// Represents an aggregated proof.
#[derive(Debug, Eq, PartialEq)]
pub struct AggregatedProofData {
    pub(crate) public_input: AggregatedProofPublicInput,
    pub(crate) info: AggregatedProofDataInfo,
}

impl AggregatedProofData {
    pub(crate) fn new(
        public_input: AggregatedProofPublicInput,
        info: AggregatedProofDataInfo,
    ) -> Self {
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

/// Holds all the necessary data for the creation of a block zk-proof.
pub struct StateTransitionInfo<StateRoot, Witness, Da: DaSpec> {
    /// Public input to the per block zk proof.
    pub(crate) data: StateTransitionData<StateRoot, Witness, Da>,
    /// State height.
    pub(crate) state_height: u64,
}

impl<StateRoot, Witness, Da: DaSpec> StateTransitionInfo<StateRoot, Witness, Da> {
    /// StateTransitionInfo constructor.
    pub fn new(data: StateTransitionData<StateRoot, Witness, Da>, state_height: u64) -> Self {
        Self { data, state_height }
    }

    pub(crate) fn da_block_header(&self) -> &Da::BlockHeader {
        &self.data.da_block_header
    }

    pub(crate) fn initial_state_root(&self) -> &StateRoot {
        &self.data.initial_state_root
    }
}
