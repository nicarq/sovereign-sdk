use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::zk::{StateTransitionPublicData, StateTransitionWitness};

pub(crate) struct BlockProof<Address, Da: DaSpec, Root> {
    pub(crate) _proof: Vec<u8>,
    pub(crate) slot_number: u64,
    pub(crate) st: StateTransitionPublicData<Address, Da, Root>,
}

/// Holds all the necessary data for the creation of a block zk-proof.
#[derive(Serialize, Deserialize)]
#[serde(bound = "StateRoot: Serialize + DeserializeOwned, Witness: Serialize + DeserializeOwned")]
pub struct StateTransitionInfo<StateRoot, Witness, Da: DaSpec> {
    /// Public input to the per block zk proof.
    pub(crate) data: StateTransitionWitness<StateRoot, Witness, Da>,
    /// Slot number.
    pub(crate) rollup_height: u64,
}

impl<StateRoot, Witness, Da: DaSpec> StateTransitionInfo<StateRoot, Witness, Da> {
    /// StateTransitionInfo constructor.
    pub fn new(data: StateTransitionWitness<StateRoot, Witness, Da>, rollup_height: u64) -> Self {
        Self {
            data,
            rollup_height,
        }
    }

    pub(crate) fn da_block_header(&self) -> &Da::BlockHeader {
        &self.data.da_block_header
    }

    pub(crate) fn initial_state_root(&self) -> &StateRoot {
        &self.data.initial_state_root
    }
}
