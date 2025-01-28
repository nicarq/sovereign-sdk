use std::net::SocketAddr;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use anyhow::Context;
use derivative::Derivative;
use sov_cli::wallet_state::PrivateKeyAndAddress;
use sov_cli::NodeClient;
use sov_db::ledger_db::LedgerDb;
use sov_mock_da::storable::service::StorableMockDaService;
use sov_mock_da::{BlockProducingConfig, MockAddress, MockDaConfig, MockDaSpec};
use sov_modules_api::execution_mode::Native;
use sov_modules_api::prelude::axum;
use sov_modules_api::prelude::axum::extract::Request;
use sov_modules_api::prelude::axum::ServiceExt;
use sov_modules_api::{Spec, Zkvm};
use sov_modules_rollup_blueprint::{FullNodeBlueprint, SequencerBlueprint};
use sov_modules_stf_blueprint::{GenesisParams, Runtime};
use sov_rollup_interface::common::SlotNumber;
use sov_rollup_interface::node::{DaSyncState, SyncStatus};
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_rollup_interface::zk::ZkvmHost;
use sov_rollup_interface::StateUpdateInfo;
use sov_sequencer::batch_builders::preferred::PreferredBatchBuilderConfig;
use sov_sequencer::batch_builders::test_stateless::TestStatelessBatchBuilder;
use sov_sequencer::{BatchBuilderConfig, SequencerConfig};
pub use sov_stf_runner::processes::RollupProverConfig;
use sov_stf_runner::{
    HttpServerConfig, MonitoringConfig, ProofManagerConfig, RollupConfig, RunnerConfig,
    StorageConfig,
};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::{TEST_DEFAULT_MOCK_DA_BLOCK_TIME_MS, TEST_DEFAULT_PROVER_ADDRESS};

/// Specifies how to source the genesis data for a rollup.
#[derive(Derivative)]
#[derivative(Clone(bound = ""))]
pub enum GenesisSource<S: Spec, R: Runtime<S>> {
    /// Genesis data will be parsed from files found at the given paths.
    ///
    /// See [`FullNodeBlueprint::create_genesis_config`].
    Paths(R::GenesisInput),
    /// Genesis data provided explicitly using [`GenesisParams`].
    ///
    /// This is most useful when you're automatically generating genesis data
    /// rather than parsing it. See e.g.
    /// [`crate::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig::generate`].
    CustomParams(GenesisParams<R::GenesisConfig>),
}

/// Various configuration options for [`RollupBuilder`].
#[allow(missing_docs)]
#[derive(Clone)]
pub struct RollupBuilderConfig<S: Spec, StoragePath = Arc<tempfile::TempDir>> {
    pub automatic_batch_production: bool,
    pub batch_builder_config: BatchBuilderConfig,
    pub prover_address: String,
    pub aggregated_proof_block_jump: usize,
    pub max_infos_in_db: u64,
    pub max_channel_size: u64,
    pub telegraf_address: SocketAddr,
    pub rollup_prover_config: RollupProverConfig<S::InnerZkvm>,
    /// This is wrapped in an [`Arc`] to enable re-use of the same directory
    /// when dropping a [`TestRollup`] and creating a new one. The pattern
    /// looks something like this:
    ///
    ///  1. Instantiate a [`RollupBuilder`].
    ///  2. Clone its [`RollupBuilderConfig::storage`] and store it for later use.
    ///  3. Run some tests against the [`TestRollup`], then call
    ///     [`TestRollup::shutdown`].
    ///  4. Instantiate a new [`RollupBuilder`] and set its
    ///     [`RollupBuilderConfig::storage`] to the original directory.
    ///  6. Voila, your data is still there and you can test node behavior after
    ///     a restart.
    pub storage: StoragePath,
    pub axum_port: u16,
}

/// A one-stop shop for building entire rollups and starting them in the
/// background to test node APIs.
#[derive(Derivative)]
#[derivative(Clone(bound = "StoragePath: Clone"))]
pub struct RollupBuilder<R: FullNodeBlueprint<Native>, StoragePath = Arc<tempfile::TempDir>> {
    genesis: GenesisSource<R::Spec, R::Runtime>,
    da_config: MockDaConfig,
    config: RollupBuilderConfig<R::Spec, StoragePath>,
    with_secondary_sequencer: Option<MockAddress>,
}

impl<R: FullNodeBlueprint<Native>> RollupBuilder<R> {
    /// Creates a new [`RollupBuilder`] with automatic [`StorableMockDaService`]
    /// configuration.
    pub fn new(
        genesis: GenesisSource<R::Spec, R::Runtime>,
        block_producing: BlockProducingConfig,
        finalization_blocks: u32,
    ) -> Self {
        Self::new_with_storage_path(
            genesis,
            block_producing,
            finalization_blocks,
            Arc::new(tempfile::tempdir().unwrap()),
        )
    }
}

/// A type that can be used as a [`Path`].
// We need a custom trait because Arc<T> doesn't implement AsRef<Path>
// even if T does.
pub trait AsPath: Clone {
    /// Returns a [`Path`] reference.
    fn as_path(&self) -> &Path;
}

// We also can't blanket impl AsPath because rustc complains that TempDir might add an `Arc<Tempdir>: AsRef<Path>` implementation in the future.
impl AsPath for Arc<tempfile::TempDir> {
    fn as_path(&self) -> &Path {
        self.as_ref().as_ref()
    }
}

impl AsPath for PathBuf {
    fn as_path(&self) -> &Path {
        self.as_path()
    }
}

impl<R: FullNodeBlueprint<Native>, StoragePath: AsPath> RollupBuilder<R, StoragePath> {
    /// Creates a new [`RollupBuilder`] with automatic [`StorableMockDaService`]
    /// configuration.
    pub fn new_with_storage_path(
        genesis: GenesisSource<R::Spec, R::Runtime>,
        block_producing: BlockProducingConfig,
        finalization_blocks: u32,
        storage_path: StoragePath,
    ) -> Self {
        let da_config = MockDaConfig {
            // This will be set later based on the storage path. In case of a bug,
            // SQLite will simply fail to open the file and we'll immediately get a
            // panic, so it's not dangerous.
            connection_string: "WILL_BE_SET_LATER".to_string(),
            // This value is important and should match `examples/test-data/genesis/integration-tests/sequencer_registry.json`
            // Otherwise batches are going to be rejected in `examples/demo-rollup` tests.
            sender_address: MockAddress::new([0; 32]),
            finalization_blocks,
            block_producing,
            block_time_ms: TEST_DEFAULT_MOCK_DA_BLOCK_TIME_MS,
            da_layer: None,
        };

        Self {
            genesis,
            da_config,
            config: RollupBuilderConfig {
                max_channel_size: 60,
                max_infos_in_db: 80 + finalization_blocks as u64,
                automatic_batch_production: true,
                batch_builder_config: BatchBuilderConfig::Preferred(Default::default()),
                prover_address: TEST_DEFAULT_PROVER_ADDRESS.to_string(),
                aggregated_proof_block_jump: 1,
                rollup_prover_config: get_appropriate_rollup_prover_config::<R::Spec>(
                    Default::default(),
                ),
                storage: storage_path,
                telegraf_address: MonitoringConfig::standard().telegraf_address,
                axum_port: 0,
            },
            with_secondary_sequencer: None,
        }
        .set_da_connection_string()
    }

    /// See [`PreferredBatchBuilderConfig::minimum_profit_per_tx`].
    pub fn with_preferred_seq_min_profit_per_tx(mut self, minimum_profit_per_tx: u64) -> Self {
        self.config.batch_builder_config =
            BatchBuilderConfig::Preferred(PreferredBatchBuilderConfig {
                minimum_profit_per_tx,
            });
        self
    }

    /// See [`RollupBuilderConfig::rollup_prover_config`].
    pub fn with_zkvm_host_args(
        mut self,
        zkvm_host_args: Arc<<<<R::Spec as Spec>::InnerZkvm as Zkvm>::Host as ZkvmHost>::HostArgs>,
    ) -> Self {
        self.config.rollup_prover_config =
            get_appropriate_rollup_prover_config::<R::Spec>(zkvm_host_args);
        self
    }

    /// Allows to modify configuration options.
    pub fn set_config(
        mut self,
        config_f: impl FnOnce(&mut RollupBuilderConfig<R::Spec, StoragePath>),
    ) -> Self {
        config_f(&mut self.config);
        // Storage path might have changed.
        self.set_da_connection_string()
    }

    /// Allows to modify DA configuration options.
    pub fn set_da_config(mut self, config_f: impl FnOnce(&mut MockDaConfig)) -> Self {
        config_f(&mut self.da_config);
        self
    }

    /// Sets the batch builder mode to [`BatchBuilderConfig::Standard`].
    pub fn with_standard_batch_builder(self) -> Self {
        self.set_config(|c| {
            c.batch_builder_config = BatchBuilderConfig::Standard(Default::default());
        })
    }

    /// Runs a secondary sequencer with [`TestStatelessBatchBuilder`] on the same DA layer
    /// with the provided DA Address.
    pub fn with_secondary_sequencer(mut self, sequencer_da_address: MockAddress) -> Self {
        self.with_secondary_sequencer = Some(sequencer_da_address);
        self
    }

    /// Returns the path that will be used for the mock DA database.
    pub fn mock_da_db_path(&self) -> PathBuf {
        self.config.storage.as_path().join("mock_da.sqlite")
    }

    /// Get a connection string for [`sov_mock_da::storable::layer::StorableMockDaLayer`].
    pub fn mock_da_connection_string(&self) -> String {
        format!("sqlite://{}?mode=rwc", self.mock_da_db_path().display())
    }

    fn set_da_connection_string(mut self) -> Self {
        // We store DA data in the same directory as the rollup data. This
        // ensures that, when reusing the same path, we restore not only node
        // data but also DA history.
        self.da_config.connection_string = self.mock_da_connection_string();
        self
    }
}

impl<R, StoragePath: AsPath> RollupBuilder<R, StoragePath>
where
    R: FullNodeBlueprint<Native, DaService = StorableMockDaService> + Default + 'static,
    R::Spec: Spec<Da = MockDaSpec>,
{
    /// Creates a new [`TestRollup`] and starts running it in a background Tokio
    /// task. See [`TestRollup`] for usage information.
    pub async fn start(self) -> anyhow::Result<TestRollup<R, StoragePath>> {
        let blueprint: R = Default::default();
        std::fs::create_dir_all(self.config.storage.as_path()).with_context(|| {
            format!(
                "Failed to create storage directory: {}",
                self.config.storage.as_path().display()
            )
        })?;

        let rollup_config = self.rollup_config();
        let rollup = match &self.genesis {
            GenesisSource::Paths(genesis_paths) => {
                blueprint
                    .create_new_rollup(
                        genesis_paths,
                        rollup_config.clone(),
                        Some(self.config.rollup_prover_config),
                    )
                    .await?
            }
            GenesisSource::CustomParams(genesis_params) => {
                blueprint
                    .create_new_rollup_with_genesis_params(
                        genesis_params.clone(),
                        rollup_config.clone(),
                        Some(self.config.rollup_prover_config),
                    )
                    .await?
            }
        };

        let (rpc_addr_tx, rpc_addr_rx) = tokio::sync::oneshot::channel();
        let (rest_addr_tx, rest_addr_rx) = tokio::sync::oneshot::channel();
        let shutdown_sender = rollup.shutdown_sender.clone();

        let da_service = rollup.runner.da_service();

        let secondary_test_sequencer_client = match self.with_secondary_sequencer {
            Some(addr) => {
                // We "keep" it because it is going to be deleted when the parent is deleted.
                let second_sequencer_dir = tempfile::Builder::new()
                    .keep(true)
                    .tempdir_in(self.config.storage.as_path())?;
                let mut rollup_config = rollup_config.clone();
                rollup_config.storage.path = second_sequencer_dir.path().to_path_buf();
                Some(
                    Self::start_secondary_sequencer(
                        da_service.another_on_the_same_layer(addr),
                        rollup_config.clone(),
                        shutdown_sender.subscribe(),
                    )
                    .await?,
                )
            }
            None => None,
        };

        let rollup_task = tokio::spawn(async move {
            rollup
                .run_and_report_addr(Some(rpc_addr_tx), Some(rest_addr_tx))
                .await?;
            tracing::info!("Completed running a rollup");
            Ok(())
        });

        let rest_addr = rest_addr_rx.await?;
        let rpc_addr = rpc_addr_rx.await?;

        let rest_port = rest_addr.port();
        let client = match NodeClient::new_at_localhost(rest_port).await {
            Ok(client) => client,
            Err(e) => {
                tracing::warn!(
                    "Unable to instantiate standard NodeClient for node at localhost:{}: {e}",
                    rest_port
                );
                NodeClient::new_at_localhost_unchecked(rest_port)
            }
        };

        Ok(TestRollup {
            rollup_task,
            api_client: sov_api_spec::client::Client::new(&client.base_url),
            rpc_addr,
            rest_addr,
            rollup_config,
            client,
            da_service,
            storage: self.config.storage.clone(),
            shutdown_sender,
            secondary_test_sequencer_client,
        })
    }

    fn rollup_config(&self) -> RollupConfig<<R::Spec as Spec>::Address, R::DaService> {
        RollupConfig {
            storage: StorageConfig {
                path: self.config.storage.as_path().to_path_buf(),
            },
            runner: RunnerConfig {
                genesis_height: 0,
                da_polling_interval_ms: 10,
                rpc_config: HttpServerConfig::localhost_on_free_port(),
                axum_config: HttpServerConfig::localhost_on_port(self.config.axum_port),
                concurrent_sync_tasks: Some(1),
            },
            da: self.da_config.clone(),
            proof_manager: ProofManagerConfig {
                aggregated_proof_block_jump: NonZero::new(self.config.aggregated_proof_block_jump)
                    .unwrap(),
                prover_address: FromStr::from_str(&self.config.prover_address)
                    .expect("Prover address is not valid"),
                max_number_of_transitions_in_db: NonZero::new(self.config.max_infos_in_db).unwrap(),
                max_number_of_transitions_in_memory: NonZero::new(self.config.max_channel_size)
                    .unwrap(),
            },
            sequencer: SequencerConfig {
                automatic_batch_production: self.config.automatic_batch_production,
                max_allowed_blocks_behind: 5,
                // Set ttl to zero to disable for testing. This prevents nondeterminism.
                dropped_tx_ttl_secs: 0,
                da_address: self.da_config.sender_address,
                admin_addresses: vec![],
                batch_builder: self.config.batch_builder_config.clone(),
            },

            monitoring: MonitoringConfig {
                telegraf_address: self.config.telegraf_address,
                max_datagram_size: None,
                max_pending_metrics: None,
            },
        }
    }

    async fn start_secondary_sequencer(
        secondary_da_service: StorableMockDaService,
        rollup_config: RollupConfig<<R::Spec as Spec>::Address, R::DaService>,
        mut shutdown_receiver: tokio::sync::watch::Receiver<()>,
    ) -> anyhow::Result<sov_api_spec::client::Client> {
        let blueprint: R = Default::default();

        let mut storage_manager = blueprint.create_storage_manager(&rollup_config)?;
        let (storage, ledger_state) = storage_manager.create_bootstrap_state()?;
        let ledger_db = LedgerDb::with_reader(ledger_state)?;

        let (sync_status_sender, _) = watch::channel(SyncStatus::START);
        let da_sync_state = Arc::new(DaSyncState {
            synced_da_height: AtomicU64::new(0),
            target_da_height: AtomicU64::new(0),
            sync_status_sender,
        });

        let state_update_info = StateUpdateInfo {
            storage: storage.clone(),
            next_event_number: 0,
            slot_number: SlotNumber::ONE,
            latest_finalized_slot_number: SlotNumber::ONE,
        };
        let (_, state_update_receiver) = watch::channel(state_update_info);

        let (sequencer, _background_handles) =
            SequencerBlueprint::<R, Native, TestStatelessBatchBuilder<R::Runtime, R::Spec>>::new(
                state_update_receiver,
                secondary_da_service,
                da_sync_state,
                &rollup_config.storage.path,
                ledger_db,
                &rollup_config.sequencer.with_bb_config(()),
                shutdown_receiver.clone(),
            )
            .await?;

        let router = sequencer.rest_api_server();

        let addr = SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0));
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let actual_address = listener.local_addr()?;
        let actual_port = actual_address.port();

        tokio::spawn(async move {
            axum::serve(listener, ServiceExt::<Request>::into_make_service(router))
                .with_graceful_shutdown(async move {
                    shutdown_receiver.changed().await.ok();
                })
                .await
        });

        Ok(sov_api_spec::client::Client::new(&format!(
            "http://127.0.0.1:{}",
            actual_port
        )))
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
pub fn get_appropriate_rollup_prover_config<S: Spec>(
    host_args: Arc<<<S::InnerZkvm as Zkvm>::Host as ZkvmHost>::HostArgs>,
) -> RollupProverConfig<S::InnerZkvm> {
    let skip_guest_build = std::env::var("SKIP_GUEST_BUILD").unwrap_or_else(|_| "0".to_string());
    if skip_guest_build == "1" {
        RollupProverConfig::Skip
    } else {
        RollupProverConfig::Execute(host_args)
    }
}

/// Represents a **running** rollup node while providing access to its
/// [`DaService`](sov_rollup_interface::node::da::DaService) and wallet client
/// to help run end-to-end tests against its APIs.
pub struct TestRollup<R: FullNodeBlueprint<Native>, StoragePath = Arc<tempfile::TempDir>> {
    /// A wallet client that can be used to interact with the node and submit
    /// txs to the sequencer.
    pub client: NodeClient,
    /// Auto-generated API client for the rollup.
    pub api_client: sov_api_spec::client::Client,
    /// Address of the JSON-RPC node server.
    pub rpc_addr: SocketAddr,
    /// Address of the REST API server.
    pub rest_addr: SocketAddr,
    /// The rollup config used to run the rollup.
    pub rollup_config: RollupConfig<<R::Spec as Spec>::Address, R::DaService>,
    /// A copy of the [`DaService`](sov_rollup_interface::node::da::DaService)
    /// that the node uses.
    ///
    /// You can use it to query DA layer information or directly submit blobs,
    /// bypassing the sequencer.
    pub da_service: Arc<StorableMockDaService>,
    /// We just hold this together with [`TestRollup`] instance, so the directory
    /// is not deleted before we're done.
    pub storage: StoragePath,
    /// Allows programmatically initialize shutdown of the test-rollup.
    /// Used for checking graceful shutdown and restart.
    pub shutdown_sender: watch::Sender<()>,
    /// Used for cleanup/shutdown logic.
    pub rollup_task: JoinHandle<anyhow::Result<()>>,
    /// In case the rollup was started with a secondary sequencer, this is the
    /// client that can be used to submit transactions.
    pub secondary_test_sequencer_client: Option<sov_api_spec::client::Client>,
}

impl<R: FullNodeBlueprint<Native>, StoragePath: AsPath> TestRollup<R, StoragePath> {
    /// Shuts down the rollup and waits for all background tasks to finish.
    pub async fn shutdown(self) -> anyhow::Result<()> {
        self.shutdown_sender
            .send(())
            .expect("Shutdown sender already closed");
        self.rollup_task.await.expect("Can't join rollup task")?;

        Ok(())
    }
}
