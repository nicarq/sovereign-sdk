#![allow(dead_code)]
use std::sync::{Arc, RwLock};

use sov_db::ledger_db::LedgerDB;
use sov_mock_da::{
    MockAddress, MockBlockHeader, MockDaConfig, MockDaService, MockDaSpec, MockDaVerifier,
    MockValidityCond,
};
use sov_mock_zkvm::{MockZkVerifier, MockZkvm};
use sov_prover_storage_manager::ProverStorageManager;
use sov_rollup_interface::rpc::{AggregatedProofResponse, LedgerRpcProvider};
use sov_rollup_interface::services::da::DaService;
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_rollup_interface::zk::aggregated_proof::AggregatedProofPublicInput;
use sov_state::{ArrayWitness, DefaultStorageSpec};
use sov_stf_runner::{
    InitVariant, ParallelProverService, ProverServiceConfig, RollupConfig, RollupProverConfig,
    RpcConfig, RunnerConfig, StateTransitionRunner, StorageConfig,
};
use tokio::sync::broadcast::error::TryRecvError;
use tokio::sync::broadcast::Receiver;

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

/// TestNode simulates a full-node.
pub struct TestNode {
    slot_subscription: Receiver<u64>,
    da: MockDaService,
    vm: MockZkvm<MockValidityCond>,
    ledger_db: LedgerDB,
}

impl TestNode {
    /// Send da transaction and optionally waits for corresponding slot/
    pub async fn send_transaction(&mut self, wait: bool) -> Result<(), anyhow::Error> {
        self.da.send_transaction(&[1, 2, 3]).await?;
        if wait {
            self.slot_subscription.recv().await?;
        }
        Ok(())
    }

    pub async fn wait_for_all_slots(&mut self) {
        loop {
            match self.slot_subscription.try_recv() {
                Ok(_) => continue,
                Err(TryRecvError::Lagged(_)) => continue,
                Err(TryRecvError::Empty) => break,
                e => panic!("Error {:?}", e),
            }
        }
    }

    pub fn make_proof(&self) {
        self.vm.make_proof();
    }

    pub async fn wait_for_aggregated_proof_in_da(&self) {
        self.da.wait_for_aggregated_proof_in_da().await;
    }

    pub fn get_latest_aggregated_proof(
        &self,
    ) -> Result<Option<AggregatedProofResponse>, anyhow::Error> {
        self.ledger_db.get_latest_aggregated_proof()
    }

    pub fn get_latest_public_input_proof(
        &self,
    ) -> Result<Option<AggregatedProofPublicInput>, anyhow::Error> {
        let proof_from_db = self.ledger_db.get_latest_aggregated_proof()?;
        Ok(proof_from_db.map(|p| p.proof.public_input().clone()))
    }
}

#[allow(clippy::type_complexity)]
pub fn initialize_runner(
    init_variant: MockInitVariant,
    aggregated_proof_block_jump: usize,
    nb_of_prover_threads: usize,
) -> (
    StateTransitionRunner<
        HashStf<MockValidityCond>,
        StorageManager,
        MockDaService,
        MockZkvm<MockValidityCond>,
        MockProverService,
    >,
    TestNode,
) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path();
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
            aggregated_proof_block_jump,
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
        nb_of_prover_threads,
        rollup_config.prover_service,
        Default::default(),
    );

    let slot_subscription = ledger_db.subscribe_slots().unwrap();
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
        TestNode {
            slot_subscription,
            da: da_service,
            vm,
            ledger_db,
        },
    )
}
