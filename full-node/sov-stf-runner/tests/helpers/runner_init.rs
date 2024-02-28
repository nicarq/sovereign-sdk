#![allow(dead_code)]
use std::sync::{Arc, RwLock};

use sov_db::ledger_db::LedgerDB;
use sov_mock_da::{
    MockAddress, MockBlockHeader, MockDaConfig, MockDaService, MockDaSpec, MockDaVerifier,
    MockValidityCond,
};
use sov_mock_zkvm::{MockZkVerifier, MockZkvm};
use sov_prover_storage_manager::ProverStorageManager;
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_state::{ArrayWitness, DefaultStorageSpec};
use sov_stf_runner::{
    InitVariant, ParallelProverService, ProverServiceConfig, RollupConfig, RollupProverConfig,
    RpcConfig, RunnerConfig, StateTransitionRunner, StorageConfig,
};

use super::TEST_CODE_COMMITMENT;
use crate::helpers::hash_stf::HashStf;

type MockInitVariant = InitVariant<HashStf<MockValidityCond>, MockZkVerifier, MockDaSpec>;
type S = DefaultStorageSpec;
type StorageManager = ProverStorageManager<MockDaSpec, S>;

pub type MockProverService = ParallelProverService<
    [u8; 32],
    ArrayWitness,
    MockDaService,
    MockZkvm<MockValidityCond>,
    HashStf<MockValidityCond>,
>;

#[allow(clippy::type_complexity)]
pub fn initialize_runner(
    path: &std::path::Path,
    init_variant: MockInitVariant,
) -> (
    StateTransitionRunner<
        HashStf<MockValidityCond>,
        StorageManager,
        MockDaService,
        MockZkvm<MockValidityCond>,
        MockProverService,
    >,
    LedgerDB,
    MockDaService,
    MockZkvm<MockValidityCond>,
) {
    let address = MockAddress::new([11u8; 32]);
    let rollup_config = RollupConfig::<MockDaConfig> {
        storage: StorageConfig {
            path: path.to_path_buf(),
        },
        runner: RunnerConfig {
            start_height: 1,
            da_polling_interval_ms: 150,
            rpc_config: RpcConfig {
                bind_host: "127.0.0.1".to_string(),
                bind_port: 0,
            },
        },
        da: MockDaConfig::instant_with_sender(address),
        prover_service: ProverServiceConfig {
            aggregated_proof_block_jump: 1,
        },
    };

    let da_service = MockDaService::new(address);

    let stf = HashStf::<MockValidityCond>::new();

    let storage_config = sov_state::config::Config {
        path: path.to_path_buf(),
    };
    let mut storage_manager = ProverStorageManager::new(storage_config).unwrap();
    let genesis_block = MockBlockHeader::from_height(0);
    let (genesis_storage, ledger_state) = storage_manager.create_state_for(&genesis_block).unwrap();

    let ledger_db = LedgerDB::with_cache_db(ledger_state).unwrap();
    let rpc_storage = Arc::new(RwLock::new(genesis_storage.clone()));

    let vm = MockZkvm::new(MockValidityCond::default());
    let verifier = MockDaVerifier::default();

    let prover_config = RollupProverConfig::Prove;

    let prover_service = ParallelProverService::new(
        vm.clone(),
        stf.clone(),
        verifier,
        prover_config,
        // Should be ZkStorage, but we don't need it for this test
        genesis_storage,
        1,
        rollup_config.prover_service,
        TEST_CODE_COMMITMENT,
    );

    (
        StateTransitionRunner::new(
            rollup_config.runner,
            da_service.clone(),
            ledger_db.clone(),
            stf,
            storage_manager,
            rpc_storage,
            init_variant,
            prover_service,
        )
        .unwrap(),
        ledger_db,
        da_service,
        vm,
    )
}
