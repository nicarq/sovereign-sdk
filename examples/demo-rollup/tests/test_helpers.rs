use std::net::SocketAddr;

use demo_stf::genesis_config::GenesisPaths;
use sov_demo_rollup::MockDemoRollup;
use sov_kernels::basic::{BasicKernelGenesisConfig, BasicKernelGenesisPaths};
use sov_mock_da::MockDaConfig;
use sov_modules_rollup_blueprint::RollupBlueprint;
use sov_stf_runner::{
    HttpServerConfig, ProofManagerConfig, RollupConfig, RollupProverConfig, RunnerConfig,
    StorageConfig,
};
use tokio::sync::oneshot;

pub async fn start_rollup(
    rpc_reporting_channel: oneshot::Sender<SocketAddr>,
    rt_genesis_paths: GenesisPaths,
    kernel_genesis_paths: BasicKernelGenesisPaths,
    rollup_prover_config: RollupProverConfig,
    da_config: MockDaConfig,
) {
    let temp_dir = tempfile::tempdir().unwrap();
    let temp_path = temp_dir.path();

    let rollup_config = RollupConfig {
        storage: StorageConfig {
            path: temp_path.to_path_buf(),
        },
        runner: RunnerConfig {
            genesis_height: 0,
            da_polling_interval_ms: 1000,
            rpc_config: HttpServerConfig {
                bind_host: "127.0.0.1".into(),
                bind_port: 0,
            },
            axum_config: HttpServerConfig {
                bind_host: "127.0.0.1".into(),
                bind_port: 0,
            },
        },
        da: da_config,
        proof_manager: ProofManagerConfig {
            aggregated_proof_block_jump: 1,
        },
    };

    let mock_demo_rollup = MockDemoRollup {};

    let kernel_genesis = BasicKernelGenesisConfig {
        chain_state: serde_json::from_reader(
            std::fs::File::open(&kernel_genesis_paths.chain_state)
                .expect("Failed to read chain_state genesis config"),
        )
        .expect("Failed to parse chain_state genesis config"),
    };

    let rollup = mock_demo_rollup
        .create_new_rollup(
            &rt_genesis_paths,
            kernel_genesis,
            rollup_config,
            Some(rollup_prover_config),
        )
        .await
        .unwrap();

    rollup
        .run_and_report_addr(Some(rpc_reporting_channel), None)
        .await
        .unwrap();
}

pub fn get_appropriate_rollup_prover_config() -> RollupProverConfig {
    let skip_guest_build = std::env::var("SKIP_GUEST_BUILD").unwrap_or_else(|_| "0".to_string());
    if skip_guest_build == "1" {
        RollupProverConfig::Skip
    } else {
        RollupProverConfig::Execute
    }
}
