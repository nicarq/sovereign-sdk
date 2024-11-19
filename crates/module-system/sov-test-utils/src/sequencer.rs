use std::net::SocketAddr;
use std::num::NonZero;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;

use sov_api_spec::Client;
use sov_db::ledger_db::LedgerDb;
use sov_db::schema::SchemaBatch;
use sov_db::storage_manager::NativeStorageManager;
use sov_mock_da::{MockAddress, MockBlock, MockDaService, MockDaSpec};
use sov_modules_api::{
    DaSyncState, RuntimeEventProcessor, RuntimeEventResponse, SlotData, SyncStatus,
};
use sov_modules_stf_blueprint::{BatchReceipt, GenesisParams, TxReceiptContents};
use sov_paymaster::{PaymasterConfig, SafeVec};
use sov_rollup_interface::node::ledger_api::LedgerStateProvider;
use sov_rollup_interface::stf::StateTransitionFunction;
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_rollup_interface::StateUpdateInfo;
use sov_sequencer::batch_builders::standard::{StdBatchBuilder, StdBatchBuilderConfig};
use sov_sequencer::batch_builders::BatchBuilder;
use sov_sequencer::{GenericSequencerSpec, SeqDbTx, Sequencer, SequencerDb};
use sov_state::{DefaultStorageSpec, ProverStorage};
use sov_value_setter::ValueSetterConfig;
use tempfile::TempDir;
use tokio::sync::watch;

use crate::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use crate::runtime::{GenesisConfig, TestOptimisticRuntime};
use crate::{TestHasher, TestPrivateKey, TestSpec, TestStfBlueprint};

type TestSequencerSpec<B> = GenericSequencerSpec<
    B,
    MockDaService,
    BatchReceipt<TestSpec>,
    TxReceiptContents<TestSpec>,
    RuntimeEventResponse<<TestOptimisticRuntime<TestSpec> as RuntimeEventProcessor>::RuntimeEvent>,
>;

/// The default test sequencer type. A [`Sequencer`] with a [`MockDaService`] for DA interactions.
pub type TestSequencer<B> = Sequencer<TestSequencerSpec<B>>;

/// The default test fair batch builder type.
/// An alias for a [`StdBatchBuilder`] with a [`TestSpec`],
/// a [`MockDaService`] for DA interactions,
/// a [`TestOptimisticRuntime`] and a [`sov_kernels::basic::BasicKernel`].
pub type TestStdBatchBuilder = StdBatchBuilder<(TestSpec, TestOptimisticRuntime<TestSpec>)>;

/// A `struct` that contains a [`Sequencer`] and a copy of its running Axum
/// server, for use in tests. See [`TestSequencerSetup::new`] and
/// [`TestSequencerSetup::with_real_batch_builder`].
pub struct TestSequencerSetup<B: BatchBuilder<Spec = TestSpec>> {
    _dir: TempDir,
    /// The [`MockDaService`] used by the [`Sequencer`].
    pub da_service: MockDaService,
    /// The [`Sequencer`] used in the test.
    pub sequencer: TestSequencer<B>,
    /// The admin private key used to create an external user account for transaction handling.
    pub admin_private_key: TestPrivateKey,
    /// The Axum server handle used to start the Axum server.
    pub axum_server_handle: axum_server::Handle,
    /// The Axum server address.
    pub axum_addr: SocketAddr,
    /// Handler for shutdown of sequencer
    pub shutdown_sender: watch::Sender<()>,
}

impl<B: BatchBuilder<Spec = TestSpec>> Drop for TestSequencerSetup<B> {
    fn drop(&mut self) {
        // Error means that senders are already shut down.
        let _ = self.shutdown_sender.send(());
        self.axum_server_handle.shutdown();
    }
}

impl<B: BatchBuilder<Spec = TestSpec>> TestSequencerSetup<B> {
    /// Like [`TestSequencerSetup::new`], but with a custom [`NativeStorageManager`].
    pub async fn with_storage_manager(
        dir: TempDir,
        da_service: MockDaService,
        batch_builder_config: B::Config,
        register_admin: bool,
        seq_db_txs: Vec<SeqDbTx>,
        mut storage_manager: NativeStorageManager<
            MockDaSpec,
            ProverStorage<DefaultStorageSpec<TestHasher>>,
        >,
    ) -> anyhow::Result<Self> {
        // Generate a genesis config, then overwrite the attester key/address with ones that
        // we know. We leave the other values untouched.
        let genesis_config =
            HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);

        let admin = genesis_config.additional_accounts[0].clone();

        let value_setter_config = ValueSetterConfig {
            admin: admin.address(),
        };
        let paymaster_config = PaymasterConfig {
            payers: SafeVec::new(),
        };

        let runtime = TestOptimisticRuntime::<TestSpec>::default();

        // Run genesis registering the attester and sequencer we've generated.
        let genesis_config = GenesisConfig::from_minimal_config(
            genesis_config.into(),
            value_setter_config,
            paymaster_config,
        );

        let params = GenesisParams {
            runtime: genesis_config,
        };

        let stf = TestStfBlueprint::with_runtime(runtime.clone());

        let genesis_block = MockBlock::default();
        let (stf_state, _ledger_state) =
            storage_manager.create_state_for(genesis_block.header())?;
        let sequencer_db = SequencerDb::new(dir.path(), Duration::ZERO)?;

        let (_genesis_root, stf_state) =
            stf.init_chain(&Default::default(), &Default::default(), stf_state, params);
        storage_manager.save_change_set(genesis_block.header(), stf_state, SchemaBatch::new())?;
        storage_manager.finalize(&genesis_block.header)?;
        let (stf_state, ledger_state) = storage_manager.create_bootstrap_state()?;
        let ledger_db = LedgerDb::with_reader(ledger_state)?;

        let (sync_status_sender, _) = watch::channel(SyncStatus::Syncing {
            synced_da_height: 0,
            target_da_height: 0,
        });

        let da_sync_state = Arc::new(DaSyncState {
            synced_da_height: AtomicU64::new(0),
            target_da_height: AtomicU64::new(0),
            sync_status_sender,
        });
        let admin_addresses = if register_admin {
            vec![admin.address()]
        } else {
            vec![]
        };

        let rollup_height = ledger_db
            .get_head_rollup_height()
            .await?
            .unwrap_or_default();
        let latest_event_number = ledger_db
            .get_latest_event_number()
            .await?
            .unwrap_or_default();

        let state_update_info = StateUpdateInfo {
            storage: stf_state,
            latest_event_number,
            rollup_height,
        };

        let (_, state_update_receiver) = watch::channel(state_update_info);
        let (shutdown_sender, mut shutdown_receiver) = watch::channel(());
        shutdown_receiver.mark_unchanged();
        let batch_builder = B::create(
            state_update_receiver,
            da_sync_state,
            da_service.sequencer_address(),
            seq_db_txs,
            admin_addresses,
            &batch_builder_config,
            0,
        )
        .await?;
        let status_manager = batch_builder.tx_status_manager();
        let (sequencer, _) = Sequencer::new(
            batch_builder,
            da_service.clone(),
            status_manager,
            sequencer_db,
            ledger_db,
            false,
            shutdown_receiver,
        );

        let (axum_addr, sequencer_axum_server) = {
            let router = sequencer.rest_api_server();
            let handle = axum_server::Handle::new();

            let handle1 = handle.clone();
            tokio::spawn(async move {
                axum_server::Server::bind(SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0)))
                    .handle(handle1)
                    .serve(router.into_make_service())
                    .await
                    .unwrap();
            });

            (handle.listening().await.unwrap(), handle)
        };

        Ok(Self {
            _dir: dir,
            da_service,
            sequencer,
            admin_private_key: admin.private_key,
            axum_server_handle: sequencer_axum_server,
            axum_addr,
            shutdown_sender,
        })
    }

    /// Instantiates a new [`Sequencer`] with a [`TestOptimisticRuntime`] and an empty
    /// [`MockDaService`].
    ///
    /// The RPC and Axum servers for the newly generated [`Sequencer`] are created
    /// on the fly, and their handles are stored inside a [`TestSequencerSetup`].
    /// This results in the automatic shutdown of the servers when the
    /// [`TestSequencerSetup`] is dropped.
    pub async fn new(
        dir: TempDir,
        da_service: MockDaService,
        batch_builder_config: B::Config,
        seq_db_txs: Vec<SeqDbTx>,
        register_admin: bool,
    ) -> anyhow::Result<Self> {
        let storage_manager = NativeStorageManager::<
            MockDaSpec,
            ProverStorage<DefaultStorageSpec<TestHasher>>,
        >::new(dir.path())?;

        Self::with_storage_manager(
            dir,
            da_service,
            batch_builder_config,
            register_admin,
            seq_db_txs,
            storage_manager,
        )
        .await
    }

    /// Returns a [`Client`] REST handler for the sequencer.
    pub fn client(&self) -> Client {
        Client::new(&format!("http://{}", self.axum_addr))
    }
}

impl TestSequencerSetup<TestStdBatchBuilder> {
    /// Like [`TestSequencerSetup::with_real_batch_builder`], but allows to
    /// specify the maximum number of transactions in the mempool before
    /// eviction.
    pub async fn with_real_batch_builder_and_mempool_max_txs_count(
        mempool_max_txs_count: NonZero<usize>,
    ) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;

        TestSequencerSetup::new(
            dir,
            MockDaService::new(MockAddress::new([172; 32])),
            StdBatchBuilderConfig {
                mempool_max_txs_count: Some(mempool_max_txs_count),
                max_batch_size_bytes: None,
            },
            vec![],
            true,
        )
        .await
    }

    /// Creates a new [`TestSequencerSetup`]. Instantiates a new [`TestOptimisticRuntime`], [`NativeStorageManager`], executes genesis
    /// and then builds a new [`StdBatchBuilder`] to instantiate a [`Sequencer`]. Instantiates an Axum server in a separate thread.
    pub async fn with_real_batch_builder() -> anyhow::Result<Self> {
        Self::with_real_batch_builder_and_mempool_max_txs_count(NonZero::new(usize::MAX).unwrap())
            .await
    }
}
