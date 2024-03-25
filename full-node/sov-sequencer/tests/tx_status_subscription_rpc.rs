use borsh::BorshSerialize;
use sov_db::sequencer_db::SequencerDB;
use sov_mock_da::{MockBlockHeader, MockDaService, MockDaSpec};
use sov_modules_api::digest::Digest;
use sov_modules_api::{Address, CryptoSpec, GasPrice, PrivateKey, Spec};
use sov_modules_stf_blueprint::kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_modules_stf_blueprint::{GenesisParams, StfBlueprint};
use sov_prover_storage_manager::ProverStorageManager;
use sov_rollup_interface::services::batch_builder::TxHash;
use sov_rollup_interface::stf::StateTransitionFunction;
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_sequencer::utils::SimpleClient;
use sov_sequencer::{FairBatchBuilder, Sequencer, TxStatus};
use sov_state::DefaultStorageSpec;
use sov_test_utils::bank_data::BankMessageGenerator;
use sov_test_utils::runtime::{create_genesis_config, ChainStateConfig, TestRuntime};
use sov_test_utils::{MessageGenerator, MockZkVerifier, TestPrivateKey, TestSpec};
use tempfile::TempDir;
use tokio::sync::watch;

type Blueprint = StfBlueprint<
    TestSpec,
    MockDaSpec,
    MockZkVerifier,
    TestRuntime<TestSpec, MockDaSpec>,
    BasicKernel<TestSpec, MockDaSpec>,
>;

fn new_sequencer(
    dir: &TempDir,
) -> Sequencer<
    FairBatchBuilder<TestSpec, MockDaSpec, TestRuntime<TestSpec, MockDaSpec>>,
    MockDaService,
> {
    let sequencer_addr = [42u8; 32];
    // Use "same" bytes for sequencer address and rollup address.
    let sequencer_rollup_addr = Address::from(sequencer_addr);
    let runtime = TestRuntime::<TestSpec, MockDaSpec>::default();

    let storage_config = sov_state::config::Config {
        path: dir.path().to_path_buf(),
    };
    let mut storage_manager =
        ProverStorageManager::<MockDaSpec, DefaultStorageSpec>::new(storage_config).unwrap();
    let genesis_block_header = MockBlockHeader::from_height(0);
    let (stf_state, ledger_storage) = storage_manager
        .create_state_for(&genesis_block_header)
        .expect("Getting genesis storage failed");

    // Config
    let token_name = "SovereignToken".to_string();

    let genesis_config = create_genesis_config(
        sequencer_rollup_addr,
        sequencer_addr.into(),
        100,
        token_name.clone(),
        10_000_000,
    );

    let blueprint = Blueprint::new();

    let kernel_genesis = BasicKernelGenesisConfig {
        chain_state: ChainStateConfig {
            current_time: Default::default(),
            gas_price_blocks_depth: 0,
            gas_price_maximum_elasticity: 0,
            initial_gas_price: GasPrice::from([15; 2]),
            minimum_gas_price: GasPrice::from([10; 2]),
        },
    };
    let params = GenesisParams {
        runtime: genesis_config,
        kernel: kernel_genesis,
    };

    let (_root_hash, change_set) = blueprint.init_chain(stf_state, params);

    storage_manager
        .save_change_set(
            &genesis_block_header,
            change_set,
            ledger_storage.clone_change_set(),
        )
        .unwrap();

    let first_block = MockBlockHeader::from_height(1);

    let sequencer_db = SequencerDB::new(dir.path()).unwrap();

    let (stf_state, _ledger_storage) = storage_manager
        .create_state_for(&first_block)
        .expect("Getting first block storage failed");

    let da_service = MockDaService::new(sequencer_addr.into());
    let batch_builder = FairBatchBuilder::new(
        usize::MAX,
        usize::MAX,
        runtime,
        watch::Sender::new(stf_state).subscribe(),
        sequencer_addr.into(),
        sequencer_db,
    )
    .unwrap();

    Sequencer::new(batch_builder, da_service)
}

#[tokio::test]
async fn subscribe() {
    let temp_dir = TempDir::new().expect("Unable to create temporary directory");
    let sequencer = new_sequencer(&temp_dir);

    let server = jsonrpsee::server::ServerBuilder::default()
        .build("127.0.0.1:0")
        .await
        .unwrap();
    let addr = server.local_addr().unwrap();
    let server_rpc_module = sequencer.rpc();
    let _server_handle = server.start(server_rpc_module);

    let client = SimpleClient::new("127.0.0.1", addr.port()).await.unwrap();

    let private_key = TestPrivateKey::generate();
    let bank_generator = BankMessageGenerator::<TestSpec>::with_minter(private_key);
    let messages_iter = bank_generator.create_messages().into_iter();
    let mut txs = Vec::default();
    for message in messages_iter {
        let tx = message.to_tx::<TestRuntime<TestSpec, MockDaSpec>>();
        txs.push(tx);
    }

    let tx_hash: TxHash = <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::Hasher::digest(
        txs[0].try_to_vec().unwrap(),
    )
    .into();
    let mut subscription = client
        .subscribe_to_tx_status_updates::<()>(tx_hash)
        .await
        .unwrap();

    let tx_status = subscription.next().await.unwrap().unwrap();
    assert_eq!(tx_status, TxStatus::Unknown);

    client.send_transactions(txs, None).await.unwrap();

    let tx_status = subscription.next().await.unwrap().unwrap();
    assert!(matches!(tx_status, TxStatus::Submitted));
    let tx_status = subscription.next().await.unwrap().unwrap();
    assert!(matches!(tx_status, TxStatus::Published { .. }));

    subscription.unsubscribe().await.unwrap();
}
