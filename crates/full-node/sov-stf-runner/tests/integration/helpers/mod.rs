use sov_stf_runner::processes::RawGenesisStateRoot;
pub mod hash_stf;
pub mod runner_init;

const GENESIS_STATE_ROOT: [u8; 32] = [22; 32];

pub fn genesis_state_root() -> RawGenesisStateRoot {
    RawGenesisStateRoot(GENESIS_STATE_ROOT.to_vec())
}
