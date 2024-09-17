use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::zk::StateTransitionPublicData;

pub(crate) struct BlockProof<Address, Da: DaSpec, Root> {
    pub(crate) _proof: Vec<u8>,
    pub(crate) slot_number: u64,
    pub(crate) st: StateTransitionPublicData<Address, Da, Root>,
}
