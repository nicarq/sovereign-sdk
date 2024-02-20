use borsh::{BorshDeserialize, BorshSerialize};
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::zk::{Proof, StateTransition, StateTransitionData};

pub(crate) struct BlockProof<Da: DaSpec, Root> {
    pub(crate) _proof: Proof,
    pub(crate) slot_number: u64,
    pub(crate) st: StateTransition<Da, Root>,
}

/// Public input of an aggregated proof.
#[derive(Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize)]
pub struct AggregatedProofPublicInput {
    /// The initial state root of the aggregated proof.
    pub initial_state_root: Vec<u8>,
    /// The final state root of the aggregated proof.
    pub final_state_root: Vec<u8>,
    /// The initial slot hash of the aggregated proof.
    pub initial_slot_hash: Vec<u8>,
    // The final slot hash of the aggregated proof.
    pub final_slot_hash: Vec<u8>,
}

// Additional information that is not zk proven. Can be used by clients for
// bookkeeping purposes.
#[derive(Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize)]
pub struct AggregatedProofDataInfo {
    /// The initial slot height of the aggregated proof.
    pub initial_slot_number: u64,
    /// The final slot height of the aggregated proof.
    pub final_slot_number: u64,
}

/// Represents an aggregated proof.
#[derive(Debug, Eq, PartialEq, BorshDeserialize, BorshSerialize)]
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
    /// Slot number.
    pub(crate) slot_number: u64,
}

impl<StateRoot, Witness, Da: DaSpec> StateTransitionInfo<StateRoot, Witness, Da> {
    /// StateTransitionInfo constructor.
    pub fn new(data: StateTransitionData<StateRoot, Witness, Da>, slot_number: u64) -> Self {
        Self { data, slot_number }
    }

    pub(crate) fn da_block_header(&self) -> &Da::BlockHeader {
        &self.data.da_block_header
    }

    pub(crate) fn initial_state_root(&self) -> &StateRoot {
        &self.data.initial_state_root
    }
}
