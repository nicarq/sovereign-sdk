use std::net::SocketAddr;

use sov_kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_mock_da::{MockBlockHeader, MockDaService, MockDaSpec, MockValidityCondChecker};
use sov_mock_zkvm::MockCodeCommitment;
use sov_modules_api::{Address, CryptoSpec, GasPrice, PrivateKey, Spec};
use sov_modules_stf_blueprint::{GenesisParams, StfBlueprint};
use sov_prover_storage_manager::ProverStorageManager;
use sov_rollup_interface::stf::StateTransitionFunction;
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_sequencer::{FairBatchBuilder, FairBatchBuilderConfig, Sequencer, SequencerDb};
use sov_state::DefaultStorageSpec;
use tempfile::TempDir;
use tokio::sync::watch;

use crate::runtime::{create_genesis_config, ChainStateConfig, TestRuntime};
use crate::{MockZkVerifier, TestHasher, TestPrivateKey, TestSpec};

const SEQUENCER_ADDR: [u8; 32] = [42u8; 32];

pub type Blueprint = StfBlueprint<
    TestSpec,
    MockDaSpec,
    MockZkVerifier,
    TestRuntime<TestSpec, MockDaSpec>,
    BasicKernel<TestSpec, MockDaSpec>,
>;

pub type TestSequencer = Sequencer<
    FairBatchBuilder<
        TestSpec,
        MockDaSpec,
        TestRuntime<TestSpec, MockDaSpec>,
        BasicKernel<TestSpec, MockDaSpec>,
    >,
    MockDaService,
>;

pub type TestCryptoSpec = <TestSpec as Spec>::CryptoSpec;
pub type AdminPrivateKey = <TestCryptoSpec as CryptoSpec>::PrivateKey;

/// A `struct` built by [`new_sequencer`] that contains a [`Sequencer`] and its
/// RPC and Axum servers.
pub struct TestSequencerSetup {
    pub sequencer: TestSequencer,
    pub admin_private_key: AdminPrivateKey,
    pub rpc_server_handle: jsonrpsee::server::ServerHandle,
    pub rpc_addr: SocketAddr,
    pub axum_server_handle: axum_server::Handle,
    pub axum_addr: SocketAddr,
}

impl Drop for TestSequencerSetup {
    fn drop(&mut self) {
        self.axum_server_handle.shutdown();
    }
}

/// Instantiates a new [`Sequencer`] with a [`TestRuntime`] and an empty
/// [`MockDaService`].
///
/// The RPC and Axum servers for the newly generated [`Sequencer`] are created
/// on the fly, and their handles are stored inside a [`TestSequencerSetup`].
/// This results in the automatic shutdown of the servers when the
/// [`TestSequencerSetup`] is dropped.
pub async fn new_sequencer(dir: &TempDir) -> anyhow::Result<TestSequencerSetup> {
    // Use "same" bytes for sequencer address and rollup address.
    let sequencer_rollup_addr = Address::from(SEQUENCER_ADDR);
    let admin_pkey = TestPrivateKey::generate();
    let runtime = TestRuntime::<TestSpec, MockDaSpec>::default();

    let storage_config = sov_state::config::Config {
        path: dir.path().to_path_buf(),
    };
    let mut storage_manager =
        ProverStorageManager::<MockDaSpec, DefaultStorageSpec>::new(storage_config)?;
    let genesis_block_header = MockBlockHeader::from_height(0);
    let (stf_state, ledger_storage) = storage_manager.create_state_for(&genesis_block_header)?;

    let genesis_config = create_genesis_config(
        admin_pkey.to_address::<TestHasher, _>(),
        sequencer_rollup_addr,
        SEQUENCER_ADDR.into(),
        100,
        "SovereignToken".to_string(),
        10_000_000,
        MockValidityCondChecker::default(),
    );

    let kernel_genesis = BasicKernelGenesisConfig {
        chain_state: ChainStateConfig {
            current_time: Default::default(),
            initial_base_fee_per_gas: GasPrice::from([15; 2]),
            inner_code_commitment: MockCodeCommitment::default(),
            outer_code_commitment: MockCodeCommitment::default(),
            genesis_da_height: 0,
        },
    };
    let params = GenesisParams {
        runtime: genesis_config,
        kernel: kernel_genesis,
    };

    let blueprint = Blueprint::new();
    let (_root_hash, change_set) = blueprint.init_chain(stf_state, params);

    storage_manager.save_change_set(
        &genesis_block_header,
        change_set,
        ledger_storage.clone_change_set(),
    )?;

    let first_block = MockBlockHeader::from_height(1);

    let sequencer_db = SequencerDb::new(dir.path())?;

    let (stf_state, _ledger_storage) = storage_manager.create_state_for(&first_block)?;

    let batch_builder_config = FairBatchBuilderConfig {
        mempool_max_txs_count: usize::MAX,
        max_batch_size_bytes: usize::MAX,
        sequencer_address: SEQUENCER_ADDR.into(),
    };
    let batch_builder = FairBatchBuilder::new(
        runtime,
        BasicKernel::default(),
        watch::Sender::new(stf_state).subscribe(),
        sequencer_db,
        batch_builder_config,
    )?;

    let da_service = MockDaService::new(SEQUENCER_ADDR.into());
    let sequencer = Sequencer::new(batch_builder, da_service);

    expose_sequencer_server(sequencer, admin_pkey).await
}

async fn expose_sequencer_server(
    sequencer: TestSequencer,
    admin_private_key: AdminPrivateKey,
) -> anyhow::Result<TestSequencerSetup> {
    let (rpc_addr, rpc_server_handle) = {
        let server = jsonrpsee::server::ServerBuilder::default()
            .build("127.0.0.1:0")
            .await?;
        let addr = server.local_addr()?;
        let server_rpc_module = sequencer.clone().rpc();

        (addr, server.start(server_rpc_module))
    };

    let (axum_addr, sequencer_axum_server) = {
        let addr = SocketAddr::from(([127, 0, 0, 1], 0));
        let router = sequencer
            .axum_router("/sequencer")
            .with_state::<()>(sequencer.clone());

        let handle = axum_server::Handle::new();
        let handle1 = handle.clone();
        tokio::spawn(async move {
            axum_server::Server::bind(addr)
                .handle(handle1)
                .serve(router.into_make_service())
                .await
                .unwrap();
        });

        (handle.listening().await.unwrap(), handle)
    };

    Ok(TestSequencerSetup {
        sequencer,
        admin_private_key,
        rpc_server_handle,
        rpc_addr,
        axum_server_handle: sequencer_axum_server,
        axum_addr,
    })
}
