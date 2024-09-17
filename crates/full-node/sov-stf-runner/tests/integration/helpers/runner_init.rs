use std::str::FromStr;
use std::sync::Arc;

use futures::stream::BoxStream;
use futures::StreamExt;
use sha2::Sha256;
use sov_db::ledger_db::LedgerDb;
use sov_db::storage_manager::NativeStorageManager;
use sov_mock_da::{
    MockBlockHeader, MockDaConfig, MockDaService, MockDaSpec, MockDaVerifier, MockFee, MockHash,
    MockValidityCond,
};
use sov_mock_zkvm::{MockZkVerifier, MockZkvm};
use sov_modules_api::{Address, Batch, FullyBakedTx, ProofSerializer};
use sov_rollup_interface::node::da::{DaService, DaServiceWithRetries};
use sov_rollup_interface::node::ledger_api::{AggregatedProofResponse, LedgerStateProvider};
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_rollup_interface::zk::aggregated_proof::{
    AggregatedProofPublicData, SerializedAggregatedProof,
};
use sov_sequencer::FairBatchBuilderConfig;
use sov_state::{DefaultStorageSpec, ProverStorage};
use sov_stf_runner::{
    new_stf_info_channel, HttpServerConfig, InitVariant, ParallelProverService, ProofManager,
    ProofManagerConfig, RollupConfig, RollupProverConfig, RunnerConfig, SequencerConfig,
    StateTransitionRunner, StorageConfig,
};
use tokio::sync::broadcast::Receiver;
use tokio::sync::watch;

use crate::helpers::hash_stf::HashStf;

type MockInitVariant = InitVariant<
    HashStf<MockValidityCond>,
    MockZkVerifier,
    MockZkVerifier,
    DaServiceWithRetries<MockDaService>,
>;
type S = DefaultStorageSpec<sha2::Sha256>;
type StorageManager = NativeStorageManager<MockDaSpec, ProverStorage<S>>;

/// TestNode simulates a full-node.
pub struct TestNode {
    proof_posted_in_da_sub: Receiver<()>,
    agg_proof_saved_in_db_sub: BoxStream<'static, AggregatedProofResponse>,
    da: Arc<DaServiceWithRetries<MockDaService>>,
    inner_vm: MockZkvm,
    _outer_vm: MockZkvm,
    ledger_db: LedgerDb,
}

impl TestNode {
    /// Creates a DA block containing a transaction blob, optionally including an aggregated proof.
    pub async fn send_transaction(&self) -> anyhow::Result<MockHash> {
        let batch = Batch::new(vec![FullyBakedTx {
            data: vec![1, 2, 3],
        }]);

        let serialized_batch = borsh::to_vec(&batch).unwrap();
        self.da
            .send_transaction(&serialized_batch, MockFee::zero())
            .await
            .map(|receipt| receipt.transaction_id)
    }

    /// Creates a DA block containing an empty transaction blob, optionally including an aggregated proof.
    pub async fn try_send_aggregated_proof(&self) -> anyhow::Result<MockHash> {
        let batch = Batch::new(vec![FullyBakedTx { data: vec![] }]);
        let serialized_batch = borsh::to_vec(&batch).unwrap();
        self.da
            .send_transaction(&serialized_batch, MockFee::zero())
            .await
            .map(|receipt| receipt.transaction_id)
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

    pub async fn get_latest_public_data(
        &self,
    ) -> anyhow::Result<Option<AggregatedProofPublicData>> {
        let proof_from_db = self.ledger_db.get_latest_aggregated_proof().await?;
        Ok(proof_from_db.map(|p| p.proof.public_data().clone()))
    }
}

struct DummyProofSerializer {}

impl ProofSerializer for DummyProofSerializer {
    fn new() -> Self {
        DummyProofSerializer {}
    }

    fn serialize_proof_blob_with_metadata(
        &self,
        serialized_proof: SerializedAggregatedProof,
    ) -> anyhow::Result<Vec<u8>> {
        Ok(serialized_proof.raw_aggregated_proof)
    }

    fn serialize_attestation_blob_with_metadata(
        &self,
        _serialized_attestation: sov_modules_api::SerializedAttestation,
    ) -> anyhow::Result<Vec<u8>> {
        unimplemented!()
    }

    fn serialize_challenge_blob_with_metadata(
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
    let rollup_config = RollupConfig::<_, MockDaConfig, FairBatchBuilderConfig<MockDaSpec>> {
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
            concurrent_sync_tasks: Some(1),
        },
        da: MockDaConfig::instant_with_sender(da_service.da_service().sequencer_address()),
        proof_manager: ProofManagerConfig {
            aggregated_proof_block_jump,
            prover_address: Address::<Sha256>::from_str(
                "sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9stup8tx",
            )
            .expect("Prover address is not valid"),
        },
        sequencer: SequencerConfig {
            max_allowed_blocks_behind: 5,
            batch_builder: FairBatchBuilderConfig {
                mempool_max_txs_count: None,
                max_batch_size_bytes: None,
                sequencer_address: da_service.da_service().sequencer_address(),
            },
        },
    };

    let stf = HashStf::<MockValidityCond>::new();

    let mut storage_manager: StorageManager = NativeStorageManager::new(path).unwrap();
    let genesis_block = MockBlockHeader::from_height(0);
    let (genesis_storage, ledger_state) = storage_manager.create_state_for(&genesis_block).unwrap();

    let mut ledger_db = LedgerDb::with_reader(ledger_state).unwrap();
    let api_storage_sender = watch::Sender::new(genesis_storage.clone());

    let inner_vm = MockZkvm::new();
    let outer_vm = MockZkvm::new_non_blocking();
    let verifier = MockDaVerifier::default();

    let (prev_state_root, genesis_state_root) = init_variant
        .calculate_initial_state_roots(&mut ledger_db, &stf, &mut storage_manager)
        .await
        .unwrap();

    let prover_config = RollupProverConfig::Prove;

    let st_info_sender = match nb_of_prover_threads {
        Some(threads) => {
            let prover_service = ParallelProverService::new(
                inner_vm.clone(),
                outer_vm.clone(),
                stf.clone(),
                verifier,
                prover_config,
                // Should be ZkStorage, but we don't need it for this test
                genesis_storage,
                threads,
                Default::default(),
                Vec::<u8>::default(),
            );

            let (st_info_sender, st_info_receiver) =
                new_stf_info_channel(ledger_db.clone(), 1, 2).await.unwrap();

            let proof_manager = ProofManager::new(
                da_service.clone(),
                prover_service,
                rollup_config.proof_manager.aggregated_proof_block_jump,
                Box::new(DummyProofSerializer::new()),
                genesis_state_root,
                st_info_receiver,
            );

            proof_manager
                .post_aggregated_proof_to_da_in_background()
                .await;

            Some(st_info_sender)
        }
        None => None,
    };

    let proof_posted_in_da_sub = da_service.da_service().subscribe_proof_posted();
    let agg_proof_saved_in_db_sub = ledger_db.subscribe_proof_saved();

    (
        StateTransitionRunner::new(
            rollup_config.runner,
            da_service.clone(),
            ledger_db.clone(),
            stf,
            storage_manager,
            api_storage_sender,
            prev_state_root,
            st_info_sender,
        )
        .await
        .unwrap(),
        TestNode {
            proof_posted_in_da_sub,
            agg_proof_saved_in_db_sub,
            da: da_service,
            inner_vm,
            _outer_vm: outer_vm,
            ledger_db,
        },
    )
}
