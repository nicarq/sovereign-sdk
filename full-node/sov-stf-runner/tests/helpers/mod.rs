use sov_rollup_interface::zk::aggregated_proof::CodeCommitment;

pub mod hash_stf;
pub mod runner_init;

/// Code commitment for testing.
pub const TEST_CODE_COMMITMENT: CodeCommitment = CodeCommitment([0; 32]);
