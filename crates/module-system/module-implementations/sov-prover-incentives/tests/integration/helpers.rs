use sov_mock_da::MockDaSpec;
use sov_prover_incentives::ProverIncentives;
use sov_test_utils::runtime::zk::genesis::HighLevelZkGenesisConfig;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{generate_zk_runtime, TestProver, TestSpec, TestUser};

pub(crate) type S = sov_test_utils::TestSpec;
pub(crate) type TestProverIncentives = ProverIncentives<S, MockDaSpec>;
pub(crate) type RT = ProverRuntime<S, MockDaSpec>;

generate_zk_runtime!(ProverRuntime <= );

pub(crate) fn setup() -> (TestRunner<RT, S>, TestProver<TestSpec>, TestUser<S>) {
    let minimal_genesis_config = HighLevelZkGenesisConfig::generate_with_additional_accounts(1);
    let unbonded_user = minimal_genesis_config
        .additional_accounts
        .first()
        .unwrap()
        .clone();
    let prover = minimal_genesis_config.initial_prover.clone();
    let genesis_config = GenesisConfig::from_minimal_config(minimal_genesis_config.into());
    let runner = TestRunner::new_with_genesis(
        genesis_config.into_genesis_params(),
        ProverRuntime::default(),
    );

    (runner, prover, unbonded_user)
}
