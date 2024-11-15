use std::marker::PhantomData;
use std::net::SocketAddr;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use sov_cli::wallet_state::PrivateKeyAndAddress;
use sov_cli::NodeClient;
use sov_mock_da::storable::service::StorableMockDaService;
use sov_mock_da::{BlockProducingConfig, MockAddress, MockDaConfig, MockDaSpec};
use sov_modules_api::execution_mode::Native;
use sov_modules_api::Spec;
use sov_modules_rollup_blueprint::{FullNodeBlueprint, Rollup};
use sov_modules_stf_blueprint::{GenesisParams, Runtime};
use sov_rollup_interface::node::da::DaServiceWithRetries;
use sov_sequencer::batch_builders::standard::StdBatchBuilderConfig;
use sov_sequencer::{BatchBuilderConfig, SequencerConfig};
use sov_stf_runner::processes::RollupProverConfig;
use sov_stf_runner::{
    HttpServerConfig, ProofManagerConfig, RollupConfig, RunnerConfig, StorageConfig,
};
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Specifies how to source the genesis data for a rollup.
pub enum GenesisSource<S: Spec, R: Runtime<S>> {
    /// Genesis data will be parsed from files found at the given paths.
    ///
    /// See [`FullNodeBlueprint::create_genesis_config`].
    Paths(R::GenesisPaths),
    /// Genesis data provided explicitly using [`GenesisParams`].
    ///
    /// This is most useful when you're automatically generating genesis data
    /// rather than parsing it. See e.g.
    /// [`crate::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig::generate`].
    CustomParams(GenesisParams<R::GenesisConfig>),
}

/// A one-stop shop for building entire rollups and starting them in the
/// background to test node APIs.
#[derive(Default)]
pub struct RollupBuilder<R> {
    phantom: PhantomData<R>,
}

impl<R> RollupBuilder<R>
where
    R: FullNodeBlueprint<Native, DaService = DaServiceWithRetries<StorableMockDaService>>
        + Default
        + 'static,
    R::Spec: Spec<Da = MockDaSpec>,
{
    /// The rollup address that is used for all [`ProofManagerConfig`]s.
    pub const PROVER_ADDRESS: &'static str =
        "sov1l6n2cku82yfqld30lanm2nfw43n2auc8clw7r5u5m6s7p8jrm4zqrr8r94";

    /// Creates a new [`Rollup`] using pre-defined, sensible defaults for
    /// configuration.
    ///
    /// Genesis initialization is automatically performed, and the prover
    /// service is started.
    pub async fn construct_rollup(
        storage_path: impl AsRef<Path>,
        genesis: GenesisSource<R::Spec, R::Runtime>,
        rollup_prover_config: RollupProverConfig,
        da_config: MockDaConfig,
    ) -> Rollup<R, Native> {
        Self::construct_rollup_and_config(storage_path, genesis, rollup_prover_config, da_config)
            .await
            .rollup
    }

    // Internal API, returns a rollup but also its config.
    async fn construct_rollup_and_config(
        storage_path: impl AsRef<Path>,
        genesis: GenesisSource<R::Spec, R::Runtime>,
        rollup_prover_config: RollupProverConfig,
        da_config: MockDaConfig,
    ) -> RollupWithConfig<R> {
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
                prover_address: FromStr::from_str(Self::PROVER_ADDRESS)
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

        let blueprint: R = Default::default();
        let rollup = match genesis {
            GenesisSource::Paths(genesis_paths) => blueprint
                .create_new_rollup(
                    &genesis_paths,
                    rollup_config.clone(),
                    Some(rollup_prover_config),
                )
                .await
                .unwrap(),
            GenesisSource::CustomParams(genesis_params) => blueprint
                .create_new_rollup_with_genesis_params(
                    genesis_params,
                    rollup_config.clone(),
                    Some(rollup_prover_config),
                )
                .await
                .unwrap(),
        };

        RollupWithConfig {
            rollup_config,
            rollup,
        }
    }

    /// Creates a new [`Rollup`] like [`RollupBuilder::construct_rollup`], but
    /// also starts running it in a background Tokio task. Node APIs are
    /// available.
    pub async fn start_rollup_in_background(
        data_path: impl AsRef<Path>,
        rpc_reporting_channel: tokio::sync::oneshot::Sender<SocketAddr>,
        rest_reporting_channel: tokio::sync::oneshot::Sender<SocketAddr>,
        genesis: GenesisSource<R::Spec, R::Runtime>,
        rollup_prover_config: RollupProverConfig,
        da_config: MockDaConfig,
    ) -> (
        RollupConfig<<R::Spec as Spec>::Address, R::DaService>,
        JoinHandle<anyhow::Result<()>>,
        Arc<R::DaService>,
        watch::Sender<()>,
    ) {
        let RollupWithConfig {
            rollup_config,
            rollup,
        } = Self::construct_rollup_and_config(data_path, genesis, rollup_prover_config, da_config)
            .await;

        let shutdown_sender = rollup.shutdown_sender.clone();

        let da_service = rollup.runner.da_service();

        (
            rollup_config,
            tokio::spawn(async move {
                rollup
                    .run_and_report_addr(Some(rpc_reporting_channel), Some(rest_reporting_channel))
                    .await?;
                tracing::info!("Completed running a rollup");
                Ok(())
            }),
            da_service,
            shutdown_sender,
        )
    }

    /// Wrapper around [`RollupBuilder::start_rollup_in_background`] with
    /// automatic [`StorableMockDaService`] configuration (an in-memory
    /// SQLite database).
    pub async fn start_memory_da_rollup_in_the_background(
        rollup_prover_config: RollupProverConfig,
        block_producing: BlockProducingConfig,
        finalization_blocks: u32,
        genesis: GenesisSource<R::Spec, R::Runtime>,
    ) -> anyhow::Result<TestRollup<R>> {
        let storage_dir = Arc::new(tempfile::tempdir()?);

        Self::start_memory_da_rollup_in_the_background_with_storage_dir(
            rollup_prover_config,
            genesis,
            storage_dir,
            block_producing,
            finalization_blocks,
            None,
        )
        .await
    }

    /// Like [`RollupBuilder::start_memory_da_rollup_in_the_background`] but
    /// with a custom storage directory.
    ///
    /// Useful for testing node restarts.
    pub async fn start_memory_da_rollup_in_the_background_with_storage_dir(
        rollup_prover_config: RollupProverConfig,
        genesis: GenesisSource<R::Spec, R::Runtime>,
        storage_dir: Arc<tempfile::TempDir>,
        block_producing: BlockProducingConfig,
        finalization_blocks: u32,
        mock_da_path: Option<&Path>,
    ) -> anyhow::Result<TestRollup<R>> {
        let (rpc_port_tx, _rpc_port_rx) = tokio::sync::oneshot::channel();
        let (rest_port_tx, rest_port_rx) = tokio::sync::oneshot::channel();

        // This value is important and should match `../test-data/genesis/integration-tests/sequencer_registry.json`
        // Otherwise batches are going to be rejected
        let sequencer_address = MockAddress::new([0; 32]);
        let block_time_ms = 100;
        let storable_mock_da_connection_string = match mock_da_path {
            None => "sqlite::memory:".to_string(),
            Some(p) => format!("sqlite://{}/mock_da.sqlite?mode=rwc", p.display()),
        };

        let mock_da_config = MockDaConfig {
            connection_string: storable_mock_da_connection_string,
            sender_address: sequencer_address,
            finalization_blocks,
            block_producing,
            block_time_ms,
        };

        let (rollup_config, rollup_task, da_service, shutdown_sender) =
            Self::start_rollup_in_background(
                storage_dir.path(),
                rpc_port_tx,
                rest_port_tx,
                genesis,
                rollup_prover_config,
                mock_da_config,
            )
            .await;

        let rest_port = rest_port_rx.await?.port();
        let client = NodeClient::new_at_localhost(rest_port).await?;

        Ok(TestRollup {
            rollup_task,
            api_client: sov_api_spec::client::Client::new(&client.base_url),
            rollup_config,
            client,
            da_service,
            storage_dir,
            shutdown_sender,
        })
    }
}

/// Reads and parses a private key from the test data directory.
pub fn read_private_key<S: Spec>(suffix: &str) -> PrivateKeyAndAddress<S> {
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

/// Parses [`RollupProverConfig`] from its env. variable.
pub fn get_appropriate_rollup_prover_config() -> RollupProverConfig {
    let skip_guest_build = std::env::var("SKIP_GUEST_BUILD").unwrap_or_else(|_| "0".to_string());
    if skip_guest_build == "1" {
        RollupProverConfig::Skip
    } else {
        RollupProverConfig::Execute
    }
}

/// Represents a **running** rollup node while providing access to its
/// [`DaService`](sov_rollup_interface::node::da::DaService) and wallet client
/// to help run end-to-end tests against its APIs.
pub struct TestRollup<R: FullNodeBlueprint<Native>> {
    /// A wallet client that can be used to interact with the node and submit
    /// txs to the sequencer.
    pub client: NodeClient,
    /// Auto-generated API client for the rollup.
    pub api_client: sov_api_spec::client::Client,
    /// The rollup config used to run the rollup.
    pub rollup_config: RollupConfig<<R::Spec as Spec>::Address, R::DaService>,
    /// A copy of the [`DaService`](sov_rollup_interface::node::da::DaService)
    /// that the node uses.
    ///
    /// You can use it to query DA layer information or directly submit blobs,
    /// bypassing the sequencer.
    pub da_service: Arc<DaServiceWithRetries<StorableMockDaService>>,
    /// We just hold this together with [`TestRollup`] instance, so the directory
    /// is not deleted before we're done.
    ///
    /// This is wrapped in an [`Arc`] to renable re-use of the same directory
    /// when dropping a [`TestRollup`] and creating a new one. The pattern
    /// looks something like this:
    ///
    ///  1. Create a [`tempfile::TempDir`] and wrap it in an [`Arc`].
    ///  2. Call e.g.
    ///     [`RollupBuilder::start_memory_da_rollup_in_the_background_with_storage_dir`]
    ///     with a cloned storage directory.
    ///  3. Drop the [`TestRollup`] instance.
    ///  4. (Optionally.) Wait some time for the rollup to shutdown and
    ///     databases to be closed.
    ///  5. Call [`RollupBuilder::start_memory_da_rollup_in_the_background_with_storage_dir`]
    ///     again with the same storage directory.
    ///  6. Voila, your data is still there and you test node behavior after a
    ///     restart.
    pub storage_dir: Arc<tempfile::TempDir>,
    /// @neysofu: used for node cleanup/shutdown logic, but I'm not sure why we
    /// need to hold on to this. TODO: docs.
    pub shutdown_sender: watch::Sender<()>,
    /// Used for cleanup/shutdown logic.
    pub rollup_task: JoinHandle<anyhow::Result<()>>,
}

struct RollupWithConfig<R: FullNodeBlueprint<Native>> {
    rollup_config: RollupConfig<<R::Spec as Spec>::Address, R::DaService>,
    rollup: Rollup<R, Native>,
}
