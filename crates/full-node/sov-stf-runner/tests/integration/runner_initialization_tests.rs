use std::sync::Arc;

use sov_mock_da::{MockAddress, MockBlock, MockDaService, MockValidityCond};
use sov_mock_zkvm::MockZkvm;
use sov_rollup_interface::node::da::DaServiceWithRetries;
use sov_stf_runner::InitVariant;

use crate::helpers::hash_stf::HashStf;
use crate::helpers::runner_init::initialize_runner;
type MockInitVariant =
    InitVariant<HashStf<MockValidityCond>, MockZkvm, MockZkvm, DaServiceWithRetries<MockDaService>>;

#[tokio::test(flavor = "multi_thread")]
async fn init_and_restart() {
    let genesis_block = MockBlock {
        header: Default::default(),
        validity_cond: Default::default(),
        batch_blobs: vec![],
        proof_blobs: vec![],
    };
    let init_variant: MockInitVariant = InitVariant::Genesis {
        block: genesis_block,
        genesis_params: vec![1],
    };

    let tmpdir = tempfile::tempdir().unwrap();
    let path = tmpdir.path();

    let da_service = Arc::new(DaServiceWithRetries::new_fast(MockDaService::new(
        MockAddress::new([11u8; 32]),
    )));

    let state_root_after_genesis = {
        let (runner, _) = initialize_runner(da_service.clone(), path, init_variant, 1, None).await;
        *runner.get_state_root()
    };

    let init_variant_2 = InitVariant::Initialized(state_root_after_genesis);

    let state_root_2 = {
        let (runner_2, _) = initialize_runner(da_service, path, init_variant_2, 1, None).await;
        *runner_2.get_state_root()
    };

    assert_eq!(state_root_after_genesis, state_root_2);
}
