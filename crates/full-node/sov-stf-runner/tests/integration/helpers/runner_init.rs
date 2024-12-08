use std::sync::Arc;

use axum::async_trait;
use futures::stream::BoxStream;
use futures::{Stream, StreamExt};
use sha2::Sha256;
use sov_db::ledger_db::LedgerDb;
use sov_db::storage_manager::NativeStorageManager;
use sov_metrics::MonitoringConfig;
use sov_mock_da::{
    MockBlockHeader, MockDaConfig, MockDaService, MockDaSpec, MockDaVerifier, MockFee, MockHash,
    MockValidityCond,
};
use sov_mock_zkvm::{MockZkvm, MockZkvmHost};
use sov_modules_api::provable_height_tracker::InfiniteHeight;
use sov_modules_api::{Batch, FullyBakedTx, ProofSerializer, StateUpdateInfo, SyncStatus};
use sov_rollup_interface::node::da::{DaService, DaServiceWithRetries};
use sov_rollup_interface::node::ledger_api::{AggregatedProofResponse, LedgerStateProvider};
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_rollup_interface::zk::aggregated_proof::SerializedAggregatedProof;
use sov_sequencer::batch_builders::standard::StdBatchBuilderConfig;
use sov_sequencer::{BatchBuilderConfig, SequencerConfig};
use sov_state::{DefaultStorageSpec, ProverStorage};
use sov_stf_runner::processes::{
    ParallelProverService, RollupProverConfig, WorkflowProcessManager,
};
use sov_stf_runner::{
    HttpServerConfig, InitVariant, ProofManagerConfig, RollupConfig, RunnerConfig,
    StateTransitionRunner, StorageConfig,
};
use tokio::sync::broadcast::Receiver;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::helpers::hash_stf::HashStf;

type MockInitVariant =
    InitVariant<HashStf<MockValidityCond>, MockZkvm, MockZkvm, DaServiceWithRetries<MockDaService>>;
type S = DefaultStorageSpec<Sha256>;
type StorageManager = NativeStorageManager<MockDaSpec, ProverStorage<S>>;

/// TestNode simulates a full-node.
pub struct TestNode {
    proof_posted_in_da_sub: Receiver<()>,
    agg_proof_saved_in_db_sub: BoxStream<'static, AggregatedProofResponse>,
    da: Arc<DaServiceWithRetries<MockDaService>>,
    inner_vm: MockZkvmHost,
    _outer_vm: MockZkvmHost,
    prover_handle: Option<JoinHandle<()>>,
    // Just to remove warnings from logs
    _sync_status_receiver: watch::Receiver<SyncStatus>,
    shutdown_sender: watch::Sender<()>,
}

impl TestNode {
    /// Creates a DA block containing a transaction blob, optionally including an aggregated proof.
    pub async fn send_transaction(&self) -> anyhow::Result<MockHash> {
        let batch = Batch::new(vec![FullyBakedTx {
            data: vec![1, 2, 3],
        }]);

        let serialized_batch = borsh::to_vec(&batch)?;
        self.da
            .send_transaction(&serialized_batch, MockFee::zero())
            .await
            .map(|receipt| receipt.da_transaction_id)
    }

    /// Creates a DA block containing an empty transaction blob, optionally including an aggregated proof.
    pub async fn try_send_aggregated_proof(&self) -> anyhow::Result<MockHash> {
        let batch = Batch::new(vec![FullyBakedTx { data: vec![] }]);
        let serialized_batch = borsh::to_vec(&batch)?;
        self.da
            .send_transaction(&serialized_batch, MockFee::zero())
            .await
            .map(|receipt| receipt.da_transaction_id)
    }

    /// Unlocks the prover service worker thread and completes the block proof.
    pub fn make_block_proof(&self) {
        self.inner_vm.make_proof();
    }

    /// The aggregated proof was posted to DA and will be included in the NEXT block.
    pub async fn wait_for_aggregated_proof_posted_to_da(&mut self) -> anyhow::Result<()> {
        Ok(self.proof_posted_in_da_sub.recv().await?)
    }

    /// The aggregated proof was saved in the db.
    pub async fn wait_for_aggregated_proof_saved_in_db(&mut self) -> AggregatedProofResponse {
        self.agg_proof_saved_in_db_sub
            .next()
            .await
            .expect("No more aggregated proofs; this is a bug, please report it")
    }

    pub async fn stop(self) {
        if let Some(handle) = self.prover_handle {
            handle.abort();
        }
        self.shutdown_sender.send(()).unwrap();
    }
}

struct DummyProofSerializer {}

#[async_trait]
impl ProofSerializer for DummyProofSerializer {
    async fn serialize_proof_blob_with_metadata(
        &self,
        serialized_proof: SerializedAggregatedProof,
    ) -> anyhow::Result<Vec<u8>> {
        Ok(serialized_proof.raw_aggregated_proof)
    }

    async fn serialize_attestation_blob_with_metadata(
        &self,
        _serialized_attestation: sov_modules_api::SerializedAttestation,
    ) -> anyhow::Result<Vec<u8>> {
        unimplemented!()
    }

    async fn serialize_challenge_blob_with_metadata(
        &self,
        _serialized_challenge: sov_modules_api::SerializedChallenge,
        _slot_height: u64,
    ) -> anyhow::Result<Vec<u8>> {
        unimplemented!()
    }
}

#[allow(clippy::type_complexity)]
pub async fn initialize_runner(
    da_service: Arc<DaServiceWithRetries<MockDaService>>,
    path: &std::path::Path,
    init_variant: MockInitVariant,
    aggregated_proof_block_jump: usize,
    nb_of_prover_threads: Option<usize>,
) -> (
    StateTransitionRunner<
        HashStf<MockValidityCond>,
        StorageManager,
        DaServiceWithRetries<MockDaService>,
        MockZkvm,
        MockZkvm,
    >,
    TestNode,
) {
    let rollup_config = rollup_config(da_service.da_service(), path, aggregated_proof_block_jump);

    let stf = HashStf::<MockValidityCond>::new();

    let mut storage_manager: StorageManager = NativeStorageManager::new(path).unwrap();
    let genesis_block = MockBlockHeader::from_height(0);
    let (genesis_storage, ledger_state) = storage_manager.create_state_for(&genesis_block).unwrap();

    let mut ledger_db = LedgerDb::with_reader(ledger_state).unwrap();

    let (state_update_sender, _state_update_recv) = {
        let rollup_height = ledger_db
            .get_head_rollup_height()
            .await
            .unwrap()
            .unwrap_or_default();
        let next_event_number = ledger_db
            .get_latest_event_number()
            .await
            .unwrap()
            .map(|x| x + 1)
            .unwrap_or_default();
        let latest_finalized_rollup_height = ledger_db
            .get_latest_finalized_rollup_height()
            .await
            .unwrap();

        watch::channel(StateUpdateInfo {
            storage: genesis_storage,
            next_event_number,
            rollup_height,
            latest_finalized_rollup_height,
        })
    };

    let (sync_sender, _sync_status_receiver) = watch::channel(SyncStatus::START);
    let (shutdown_sender, mut shutdown_receiver) = watch::channel(());
    shutdown_receiver.mark_unchanged();

    let inner_vm = MockZkvmHost::new();
    let outer_vm = MockZkvmHost::new_non_blocking();
    let verifier = MockDaVerifier::default();

    let (prev_state_root, genesis_state_root) = init_variant
        .initialize(&mut ledger_db, &stf, &mut storage_manager)
        .await
        .unwrap();

    let mut runner = StateTransitionRunner::new(
        rollup_config.runner.clone(),
        if nb_of_prover_threads.is_some() {
            Some(rollup_config.proof_manager)
        } else {
            None
        },
        da_service.clone(),
        ledger_db.clone(),
        stf,
        storage_manager,
        state_update_sender,
        prev_state_root,
        sync_sender,
        Box::new(InfiniteHeight),
        shutdown_receiver.clone(),
        rollup_config.monitoring.clone(),
    )
    .await
    .unwrap();

    let handle = if let Some(st_info_receiver) = runner.take_st_info_receiver() {
        let prover_service = ParallelProverService::<_, _, _, _, MockZkvm, MockZkvm>::new(
            inner_vm.clone(),
            outer_vm.clone(),
            verifier,
            RollupProverConfig::Prove,
            nb_of_prover_threads.unwrap(),
            Default::default(),
            Vec::<u8>::default(),
        );

        let process_manager = WorkflowProcessManager::new(
            prover_service,
            da_service.clone(),
            genesis_state_root,
            shutdown_receiver.clone(),
            st_info_receiver,
            Box::new(DummyProofSerializer {}),
        );

        let handle = process_manager
            .start_zk_workflow_in_background(
                rollup_config.proof_manager.aggregated_proof_block_jump,
            )
            .await
            .unwrap();

        Some(handle)
    } else {
        None
    };

    let proof_posted_in_da_sub = da_service.da_service().subscribe_proof_posted();
    let agg_proof_saved_in_db_sub: std::pin::Pin<
        Box<dyn Stream<Item = AggregatedProofResponse> + Send>,
    > = ledger_db.subscribe_proof_saved();

    (
        runner,
        TestNode {
            proof_posted_in_da_sub,
            agg_proof_saved_in_db_sub,
            da: da_service,
            inner_vm,
            _outer_vm: outer_vm,
            prover_handle: handle,
            shutdown_sender,
            _sync_status_receiver,
        },
    )
}

fn rollup_config(
    da_service: &MockDaService,
    path: &std::path::Path,
    aggregated_proof_block_jump: usize,
) -> RollupConfig<[u8; 32], MockDaService> {
    RollupConfig {
        storage: StorageConfig {
            path: path.to_path_buf(),
        },
        runner: RunnerConfig {
            genesis_height: 0,
            da_polling_interval_ms: 150,
            rpc_config: HttpServerConfig::localhost_on_free_port(),
            axum_config: HttpServerConfig::localhost_on_free_port(),
            concurrent_sync_tasks: Some(1),
        },
        da: MockDaConfig::instant_with_sender(da_service.sequencer_address()),
        proof_manager: ProofManagerConfig {
            aggregated_proof_block_jump,
            prover_address: [0u8; 32],
            max_number_of_transitions_in_db: 30,
            max_number_of_transitions_in_memory: 20,
        },
        sequencer: SequencerConfig {
            automatic_batch_production: false,
            max_allowed_blocks_behind: 5,
            // Set ttl to zero to disable for testing. This prevents nondeterminism.
            dropped_tx_ttl_secs: 0,
            admin_addresses: vec![],
            da_address: da_service.sequencer_address(),
            batch_builder: BatchBuilderConfig::Standard(StdBatchBuilderConfig {
                mempool_max_txs_count: None,
                max_batch_size_bytes: None,
            }),
        },
        monitoring: MonitoringConfig::standard(),
    }
}
