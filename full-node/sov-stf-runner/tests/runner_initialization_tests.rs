use sov_mock_da::{MockBlock, MockDaService, MockValidityCond};
use sov_mock_zkvm::MockZkVerifier;
use sov_stf_runner::InitVariant;
mod helpers;
use helpers::runner_init::initialize_runner;

use crate::helpers::hash_stf::HashStf;
type MockInitVariant = InitVariant<HashStf<MockValidityCond>, MockZkVerifier, MockDaService>;

#[tokio::test]
async fn init_and_restart() {
    let genesis_block = MockBlock {
        header: Default::default(),
        validity_cond: Default::default(),
        blobs: vec![],
    };
    let init_variant: MockInitVariant = InitVariant::Genesis {
        block: genesis_block,
        genesis_params: vec![1],
    };

    let state_root_after_genesis = {
        let (runner, _) = initialize_runner(init_variant, 1, 1);
        *runner.get_state_root()
    };

    let init_variant_2 = InitVariant::Initialized(state_root_after_genesis);

    let state_root_2 = {
        let (runner_2, _) = initialize_runner(init_variant_2, 1, 1);
        *runner_2.get_state_root()
    };

    assert_eq!(state_root_after_genesis, state_root_2);
}
