use std::sync::Arc;

use sov_mock_da::BlockProducingConfig;
use sov_modules_api::Runtime;
use sov_modules_stf_blueprint::GenesisParams;
use sov_stf_runner::processes::RollupProverConfig;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::test_rollup::{GenesisSource, RollupBuilder};
use sov_test_utils::{generate_optimistic_runtime, RtAgnosticBlueprint, TestSpec};
use tempfile::TempDir;

generate_optimistic_runtime!(TestRuntime <=);

type TestBlueprint = RtAgnosticBlueprint<TestSpec, TestRuntime<TestSpec>>;

#[tokio::test(flavor = "multi_thread")]
async fn flaky_test_rollup_shutdown_works_as_expected() {
    let dir = Arc::new(tempfile::tempdir().unwrap());

    start_and_stop_node_in_dir(dir.clone()).await;
    start_and_stop_node_in_dir(dir.clone()).await;

    Arc::into_inner(dir)
        .expect("Someone is still holding on to the directory, but everything was shutdown.")
        .close()
        .expect("Node storage directory didn't close successfully.");
}

async fn start_and_stop_node_in_dir(dir: Arc<TempDir>) {
    let genesis_params = GenesisParams {
        runtime: <TestRuntime<TestSpec> as Runtime<TestSpec>>::GenesisConfig::from_minimal_config(
            HighLevelOptimisticGenesisConfig::generate().into(),
        ),
    };

    let test_rollup = RollupBuilder::<TestBlueprint>::new(
        GenesisSource::CustomParams(genesis_params),
        BlockProducingConfig::Periodic,
        1,
        0,
        Default::default(),
    )
    .set_config(|c| {
        c.storage = dir;
        c.rollup_prover_config = RollupProverConfig::Skip;
    })
    .with_standard_batch_builder()
    .start()
    .await
    .unwrap();

    test_rollup.shutdown().await.unwrap();
}
