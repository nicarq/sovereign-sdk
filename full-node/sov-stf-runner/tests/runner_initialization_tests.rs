use sov_mock_da::{MockBlockHeader, MockDaSpec, MockValidityCond};
use sov_mock_zkvm::MockZkVerifier;
use sov_stf_runner::InitVariant;

mod helpers;
use helpers::hash_stf::HashStf;
use helpers::runner_init::initialize_runner;
type MockInitVariant = InitVariant<HashStf<MockValidityCond>, MockZkVerifier, MockDaSpec>;

#[tokio::test]
async fn init_and_restart() {
    let tmpdir = tempfile::tempdir().unwrap();
    let genesis_params = vec![1, 2, 3, 4, 5];
    let init_variant: MockInitVariant = InitVariant::Genesis {
        block_header: MockBlockHeader::from_height(0),
        genesis_params,
    };

    let state_root_after_genesis = {
        let (runner, _, _, _) = initialize_runner(tmpdir.path(), init_variant);
        *runner.get_state_root()
    };

    let init_variant_2: MockInitVariant = InitVariant::Initialized(state_root_after_genesis);

    let (runner_2, _, _, _) = initialize_runner(tmpdir.path(), init_variant_2);

    let state_root_2 = *runner_2.get_state_root();

    assert_eq!(state_root_after_genesis, state_root_2);
}
