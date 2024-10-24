use std::net::SocketAddr;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use demo_stf::genesis_config::GenesisPaths;
use sha2::Sha256;
use sov_cli::wallet_state::PrivateKeyAndAddress;
use sov_cli::NodeClient;
use sov_demo_rollup::MockDemoRollup;
use sov_mock_da::storable::service::StorableMockDaService;
use sov_mock_da::{BlockProducingConfig, MockAddress, MockDaConfig};
use sov_modules_api::execution_mode::Native;
use sov_modules_api::{Address, OperatingMode, Spec};
use sov_modules_rollup_blueprint::{FullNodeBlueprint, Rollup};
use sov_rollup_interface::node::da::DaServiceWithRetries;
use sov_sequencer::batch_builders::standard::StdBatchBuilderConfig;
use sov_sequencer::{BatchBuilderConfig, SequencerConfig};
use sov_stf_runner::processes::RollupProverConfig;
use sov_stf_runner::{
    HttpServerConfig, ProofManagerConfig, RollupConfig, RunnerConfig, StorageConfig,
};
use tokio::task::JoinHandle;

const PROVER_ADDRESS: &str = "sov1l6n2cku82yfqld30lanm2nfw43n2auc8clw7r5u5m6s7p8jrm4zqrr8r94";

pub fn read_private_keys<S: Spec>(suffix: &str) -> PrivateKeyAndAddress<S> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();

    let private_keys_dir = Path::new(&manifest_dir).join("../test-data/keys");

    let data = std::fs::read_to_string(private_keys_dir.join(suffix))
        .expect("Unable to read file to string");

    let key_and_address: PrivateKeyAndAddress<S> =
        serde_json::from_str(&data).unwrap_or_else(|_| {
            panic!("Unable to convert data {} to PrivateKeyAndAddress", &data);
        });

    assert!(
        key_and_address.is_matching_to_default(),
        "Inconsistent key data"
    );

    key_and_address
}

pub async fn construct_rollup(
    storage_path: impl AsRef<Path>,
    rt_genesis_paths: GenesisPaths,
    rollup_prover_config: RollupProverConfig,
    da_config: MockDaConfig,
) -> Rollup<MockDemoRollup<Native>, Native> {
    let sequencer_address = da_config.sender_address;

    let rollup_config = RollupConfig {
        storage: StorageConfig {
            path: storage_path.as_ref().to_path_buf(),
        },
        runner: RunnerConfig {
            genesis_height: 0,
            da_polling_interval_ms: 10,
            rpc_config: HttpServerConfig::localhost_on_free_port(),
            axum_config: HttpServerConfig::localhost_on_free_port(),
            concurrent_sync_tasks: Some(1),
        },
        da: da_config,
        proof_manager: ProofManagerConfig {
            aggregated_proof_block_jump: 1,
            prover_address: Address::<Sha256>::from_str(PROVER_ADDRESS)
                .expect("Prover address is not valid"),
        },
        sequencer: SequencerConfig {
            automatic_batch_production: false,
            max_allowed_blocks_behind: 5,
            // Set ttl to zero to disable for testing. This prevents nondeterminism.
            dropped_tx_ttl_secs: 0,
            da_address: sequencer_address,
            batch_builder: BatchBuilderConfig::standard(StdBatchBuilderConfig {
                mempool_max_txs_count: None,
                max_batch_size_bytes: None,
            }),
        },
    };

    let mock_demo_rollup = MockDemoRollup::<Native>::default();

    mock_demo_rollup
        .create_new_rollup(&rt_genesis_paths, rollup_config, Some(rollup_prover_config))
        .await
        .unwrap()
}

pub async fn start_rollup_in_background(
    rpc_reporting_channel: tokio::sync::oneshot::Sender<SocketAddr>,
    rest_reporting_channel: tokio::sync::oneshot::Sender<SocketAddr>,
    rt_genesis_paths: GenesisPaths,
    rollup_prover_config: RollupProverConfig,
    da_config: MockDaConfig,
) -> (
    JoinHandle<()>,
    Arc<<MockDemoRollup<Native> as FullNodeBlueprint<Native>>::DaService>,
    tempfile::TempDir,
) {
    let temp_dir = tempfile::tempdir().unwrap();
    let rollup: Rollup<MockDemoRollup<Native>, Native> = construct_rollup(
        temp_dir.path(),
        rt_genesis_paths,
        rollup_prover_config,
        da_config,
    )
    .await;

    let da_service = rollup.runner.da_service();

    (
        tokio::spawn(async move {
            rollup
                .run_and_report_addr(Some(rpc_reporting_channel), Some(rest_reporting_channel))
                .await
                .unwrap();
        }),
        da_service,
        temp_dir,
    )
}

pub fn get_appropriate_rollup_prover_config() -> RollupProverConfig {
    let skip_guest_build = std::env::var("SKIP_GUEST_BUILD").unwrap_or_else(|_| "0".to_string());
    if skip_guest_build == "1" {
        RollupProverConfig::Skip
    } else {
        RollupProverConfig::Execute
    }
}

pub fn test_genesis_paths(operating_mode: OperatingMode) -> GenesisPaths {
    let dir: &dyn AsRef<Path> = &"../test-data/genesis/integration-tests/";
    GenesisPaths {
        bank_genesis_path: dir.as_ref().join("bank.json"),
        sequencer_genesis_path: dir.as_ref().join("sequencer_registry.json"),
        value_setter_genesis_path: dir.as_ref().join("value_setter.json"),
        accounts_genesis_path: dir.as_ref().join("accounts.json"),
        prover_incentives_genesis_path: dir.as_ref().join("prover_incentives.json"),
        attester_incentives_genesis_path: dir.as_ref().join("attester_incentives.json"),
        nft_path: dir.as_ref().join("nft.json"),
        evm_genesis_path: dir.as_ref().join("evm.json"),
        chain_state_genesis_path: {
            match operating_mode {
                OperatingMode::Zk => dir.as_ref().join("chain_state_zk.json"),
                OperatingMode::Optimistic => dir.as_ref().join("chain_state_op.json"),
            }
        },
    }
}

pub struct TestRollup {
    pub rollup_task: JoinHandle<()>,
    pub client: NodeClient,
    pub da_service: Arc<DaServiceWithRetries<StorableMockDaService>>,
    // We just hold it together with test rollup, so it is not removed earlier than rollup stopped.
    #[allow(dead_code)]
    storage_dir: tempfile::TempDir,
}

impl TestRollup {
    pub async fn create_test_rollup(
        rollup_prover_config: RollupProverConfig,
        block_producing: BlockProducingConfig,
        finalization_blocks: u32,
        operating_mode: OperatingMode,
    ) -> anyhow::Result<TestRollup> {
        let (rpc_port_tx, _rpc_port_rx) = tokio::sync::oneshot::channel();
        let (rest_port_tx, rest_port_rx) = tokio::sync::oneshot::channel();

        // This value is important and should match ../test-data/genesis/integration-tests /sequencer_registry.json
        // Otherwise batches are going to be rejected
        let sequencer_address = MockAddress::new([0; 32]);
        let block_time_ms = 100;
        let storable_mock_da_connection_string = "sqlite::memory:".to_string();

        let mock_da_config = MockDaConfig {
            connection_string: storable_mock_da_connection_string,
            sender_address: sequencer_address,
            finalization_blocks,
            block_producing,
            block_time_ms,
        };

        let rt_genesis_paths = test_genesis_paths(operating_mode);

        let (rollup_task, da_service, storage_dir) = start_rollup_in_background(
            rpc_port_tx,
            rest_port_tx,
            rt_genesis_paths,
            rollup_prover_config,
            mock_da_config,
        )
        .await;

        let rest_port = rest_port_rx.await?.port();
        let client = NodeClient::new_at_localhost(rest_port).await?;

        Ok(TestRollup {
            rollup_task,
            client,
            da_service,
            storage_dir,
        })
    }
}
