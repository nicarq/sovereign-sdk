use sov_mock_da::MockBlockHeader;
use sov_stf_runner::InitVariant;
mod helpers;
use helpers::runner_init::initialize_runner;

#[tokio::test]
async fn init_and_restart() {
    let init_variant = InitVariant::Genesis {
        block_header: MockBlockHeader::from_height(0),
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
