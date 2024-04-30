#![allow(dead_code)]

use std::sync::Arc;

use sov_db::ledger_db::LedgerDb;
use sov_mock_da::{
    MockBlockHeader, MockDaConfig, MockDaService, MockDaSpec, MockDaVerifier, MockFee,
    MockValidityCond,
};
use sov_mock_zkvm::{MockCodeCommitment, MockZkVerifier, MockZkvm};
use sov_prover_storage_manager::ProverStorageManager;
use sov_rollup_interface::rpc::{AggregatedProofResponse, LedgerStateProvider};
use sov_rollup_interface::services::da::DaService;
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_rollup_interface::zk::aggregated_proof::AggregatedProofPublicData;
use sov_state::{ArrayWitness, DefaultStorageSpec};
use sov_stf_runner::{
    HttpServerConfig, InitVariant, ParallelProverService, ProofManager, ProofManagerConfig,
    RollupConfig, RollupProverConfig, RunnerConfig, StateTransitionRunner, StorageConfig,
};
use tokio::sync::broadcast::Receiver;
use tokio::sync::watch;

use crate::helpers::hash_stf::HashStf;

type MockInitVariant = InitVariant<HashStf<MockValidityCond>, MockZkVerifier, MockDaService>;
type S = DefaultStorageSpec;
type StorageManager = ProverStorageManager<MockDaSpec, S>;

pub type MockProverService = ParallelProverService<
    [u8; 32],
    ArrayWitness,
    MockDaService,
    MockZkvm,
    MockZkvm,
    HashStf<MockValidityCond>,
>;

/// TestNode simulates a full-node.
pub struct TestNode {
    proof_posted_in_da_sub: Receiver<()>,
    agg_proof_saved_in_db_sub: Receiver<AggregatedProofResponse>,
    da: Arc<MockDaService>,
    inner_vm: MockZkvm,
    outer_vm: MockZkvm,
    ledger_db: LedgerDb,
}

impl TestNode {
    /// Creates a DA block containing a transaction blob, optionally including an aggregated proof.
    pub async fn send_transaction(&self) -> Result<(), anyhow::Error> {
        self.da.send_transaction(&[1, 2, 3], MockFee::zero()).await
    }

    /// Creates a DA block containing an empty transaction blob, optionally including an aggregated proof.  
    pub async fn try_send_aggregated_proof(&self) -> Result<(), anyhow::Error> {
        self.da.send_transaction(&[], MockFee::zero()).await
    }

    /// Unlocks the prover service worker thread and completes the block proof.
    pub fn make_block_proof(&self) {
        self.inner_vm.make_proof();
    }

    /// The aggregated proof was posted to DA and will be included in the NEXT block.
    pub async fn wait_for_aggregated_proof_posted_to_da(&mut self) -> Result<(), anyhow::Error> {
        Ok(self.proof_posted_in_da_sub.recv().await?)
    }

    /// The aggregated proof was saved in the db.
    pub async fn wait_for_aggregated_proof_saved_in_db(
        &mut self,
    ) -> Result<AggregatedProofResponse, anyhow::Error> {
        Ok(self.agg_proof_saved_in_db_sub.recv().await?)
    }

    /// The latest aggregated proof saved in the db.
    pub async fn get_latest_aggregated_proof(
        &self,
    ) -> Result<Option<AggregatedProofResponse>, anyhow::Error> {
        self.ledger_db.get_latest_aggregated_proof().await
    }

    pub async fn get_latest_public_data(
        &self,
    ) -> Result<Option<AggregatedProofPublicData>, anyhow::Error> {
        let proof_from_db = self.ledger_db.get_latest_aggregated_proof().await?;
        Ok(proof_from_db.map(|p| p.proof.public_data().clone()))
    }
}

pub fn initialize_runner(
    da_service: Arc<MockDaService>,
    path: &std::path::Path,
    init_variant: MockInitVariant,
    aggregated_proof_block_jump: usize,
    nb_of_prover_threads: Option<usize>,
) -> (
    StateTransitionRunner<
        HashStf<MockValidityCond>,
        StorageManager,
        MockDaService,
        MockZkvm,
        MockProverService,
    >,
    TestNode,
) {
    let rollup_config = RollupConfig::<MockDaConfig> {
        storage: StorageConfig {
            path: path.to_path_buf(),
        },
        runner: RunnerConfig {
            genesis_height: 0,
            da_polling_interval_ms: 150,
            rpc_config: HttpServerConfig {
                bind_host: "127.0.0.1".to_string(),
                bind_port: 0,
            },
            axum_config: HttpServerConfig {
                bind_host: "127.0.0.1".to_string(),
                bind_port: 0,
            },
        },
        da: MockDaConfig::instant_with_sender(da_service.sequencer_address()),
        proof_manager: ProofManagerConfig {
            aggregated_proof_block_jump,
        },
    };

    let stf = HashStf::<MockValidityCond>::new();

    let storage_config = sov_state::config::Config {
        path: path.to_path_buf(),
    };
    let mut storage_manager = ProverStorageManager::new(storage_config).unwrap();
    let genesis_block = MockBlockHeader::from_height(0);
    let (genesis_storage, ledger_state) = storage_manager.create_state_for(&genesis_block).unwrap();

    let ledger_db = LedgerDb::with_cache_db(ledger_state).unwrap();
    let rpc_storage_sender = watch::Sender::new(genesis_storage.clone());

    let inner_vm = MockZkvm::new();
    let outer_vm = MockZkvm::new_non_blocking();
    let verifier = MockDaVerifier::default();

    let prover_config = RollupProverConfig::Prove;

    let prover_service = nb_of_prover_threads.map(|threads| {
        ParallelProverService::new(
            inner_vm.clone(),
            outer_vm.clone(),
            stf.clone(),
            verifier,
            prover_config,
            // Should be ZkStorage, but we don't need it for this test
            genesis_storage,
            threads,
            Default::default(),
        )
    });

    let proof_posted_in_da_sub = da_service.subscribe_proof_posted();
    let agg_proof_saved_in_db_sub = ledger_db.subscribe_proof_saved();

    let proof_manager = ProofManager::new(
        da_service.clone(),
        prover_service,
        ledger_db.clone(),
        MockCodeCommitment::default(),
        rollup_config.proof_manager,
    );
    (
        StateTransitionRunner::new(
            rollup_config.runner,
            da_service.clone(),
            ledger_db.clone(),
            stf,
            storage_manager,
            rpc_storage_sender,
            init_variant,
            proof_manager,
        )
        .unwrap(),
        TestNode {
            proof_posted_in_da_sub,
            agg_proof_saved_in_db_sub,
            da: da_service,
            inner_vm,
            outer_vm,
            ledger_db,
        },
    )
}
