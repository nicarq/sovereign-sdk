use std::num::NonZero;
use std::sync::Arc;

use axum::async_trait;
use futures::stream::BoxStream;
use futures::{Stream, StreamExt};
use rockbound::SchemaBatch;
use sha2::Sha256;
use sov_db::ledger_db::LedgerDb;
use sov_db::schema::DeltaReader;
use sov_db::storage_manager::NativeStorageManager;
use sov_metrics::MonitoringConfig;
use sov_mock_da::{
    BlockProducingConfig, MockAddress, MockDaConfig, MockDaService, MockDaSpec, MockDaVerifier,
    MockFee, MockHash,
};
use sov_mock_zkvm::{MockZkvm, MockZkvmHost};
use sov_modules_api::provable_height_tracker::InfiniteHeight;
use sov_modules_api::{
    FullyBakedTx, ProofSerializer, StateTransitionFunction, StateUpdateInfo, SyncStatus,
};
use sov_rollup_interface::common::SlotNumber;
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::node::ledger_api::{AggregatedProofResponse, LedgerStateProvider};
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_rollup_interface::zk::aggregated_proof::SerializedAggregatedProof;
use sov_rollup_interface::zk::Zkvm;
use sov_sequencer::standard::StdSequencerConfig;
use sov_sequencer::{SequencerConfig, SequencerKindConfig};
use sov_state::{DefaultStorageSpec, NativeStorage, ProverStorage};
use sov_stf_runner::processes::{
    ParallelProverService, RollupProverConfigDiscriminants, WorkflowProcessManager,
};
use sov_stf_runner::{
    initialize_state, HttpServerConfig, ProofManagerConfig, RollupConfig, RunnerConfig,
    StateTransitionRunner, StorageConfig,
};
use tokio::sync::broadcast::Receiver;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::helpers::hash_stf::HashStf;

type MockInitVariant = InitVariant<HashStf, MockZkvm, MockZkvm, MockDaService>;

pub type S = DefaultStorageSpec<Sha256>;
pub type StorageManager = NativeStorageManager<MockDaSpec, ProverStorage<S>>;
pub type HashStfRunner<Da> = StateTransitionRunner<HashStf, StorageManager, Da, MockZkvm, MockZkvm>;

/// TestNode simulates a full-node.
pub struct TestNode {
    proof_posted_in_da_sub: Receiver<()>,
    agg_proof_saved_in_db_sub: BoxStream<'static, AggregatedProofResponse>,
    da: Arc<MockDaService>,
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
        let batch = vec![FullyBakedTx {
            data: vec![1, 2, 3],
        }];

        let serialized_batch = borsh::to_vec(&batch)?;
        self.da
            .send_transaction(&serialized_batch, MockFee::zero())
            .await
            .await?
            .map(|receipt| receipt.da_transaction_id)
    }

    /// Creates a DA block containing an empty transaction blob, optionally including an aggregated proof.
    pub async fn try_send_aggregated_proof(&self) -> anyhow::Result<MockHash> {
        let batch = vec![FullyBakedTx { data: vec![] }];
        let serialized_batch = borsh::to_vec(&batch)?;
        self.da
            .send_transaction(&serialized_batch, MockFee::zero())
            .await
            .await?
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
        _slot_height: SlotNumber,
    ) -> anyhow::Result<Vec<u8>> {
        unimplemented!()
    }
}

// Returns genesis state root, prev state root for given init variant and initial value for state update info.
pub async fn bootstrap_state_update_info(
    storage_manager: &mut StorageManager,
) -> anyhow::Result<StateUpdateInfo<ProverStorage<S>>> {
    let (stf_storage, ledger_state) = storage_manager.create_bootstrap_state()?;
    let ledger_db = LedgerDb::with_reader(ledger_state)?;

    let state_update_info = {
        let slot_number = ledger_db.get_head_slot_number().await?.unwrap_or_default();
        let next_event_number = ledger_db
            .get_latest_event_number()
            .await?
            .map(|x| x + 1)
            .unwrap_or_default();
        let latest_finalized_slot_number = ledger_db.get_latest_finalized_slot_number().await?;

        StateUpdateInfo {
            storage: stf_storage,
            next_event_number,
            slot_number,
            latest_finalized_slot_number,
        }
    };

    Ok(state_update_info)
}

pub async fn initialize_runner(
    da_service: Arc<MockDaService>,
    path: &std::path::Path,
    init_variant: MockInitVariant,
    aggregated_proof_block_jump: usize,
    nb_of_prover_threads: Option<usize>,
) -> (HashStfRunner<MockDaService>, TestNode) {
    let stf = HashStf::new();
    let inner_vm = MockZkvmHost::new();
    let outer_vm = MockZkvmHost::new_non_blocking();
    let verifier = MockDaVerifier::default();

    let rollup_config = rollup_config(&da_service, path, aggregated_proof_block_jump);
    let mut storage_manager: StorageManager = NativeStorageManager::new(path).unwrap();

    let (state_update_sender, _state_update_recv) = watch::channel(
        bootstrap_state_update_info(&mut storage_manager)
            .await
            .unwrap(),
    );

    let (sync_sender, _sync_status_receiver) = watch::channel(SyncStatus::START);
    let (shutdown_sender, mut shutdown_receiver) = watch::channel(());
    shutdown_receiver.mark_unchanged();

    let (prev_state_root, genesis_state_root) = init_variant
        .initialize(&stf, &mut storage_manager)
        .await
        .unwrap();

    let (_, ledger_state) = storage_manager.create_bootstrap_state().unwrap();
    let ledger_db = LedgerDb::with_reader(ledger_state).unwrap();
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
            RollupProverConfigDiscriminants::Prove,
            nb_of_prover_threads.unwrap(),
            Default::default(),
            MockAddress::new([0u8; 32]),
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

    let proof_posted_in_da_sub = da_service.subscribe_proof_posted();
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

type GenesisParams<ST, InnerVm, OuterVm, Da> =
    <ST as StateTransitionFunction<InnerVm, OuterVm, Da>>::GenesisParams;

/// How [`StateTransitionRunner`] is initialized
pub enum InitVariant<
    Stf: StateTransitionFunction<InnerVm, OuterVm, Da::Spec>,
    InnerVm: Zkvm,
    OuterVm: Zkvm,
    Da: DaService,
> {
    /// From give state root
    Initialized(Stf::StateRoot),
    /// From empty state root
    Genesis {
        /// Genesis block header should be finalized at an initialization moment.
        block: Da::FilteredBlock,
        /// Genesis params for Stf::init.
        genesis_params: GenesisParams<Stf, InnerVm, OuterVm, Da::Spec>,
    },
}

impl<Stf, InnerVm, OuterVm, Da> InitVariant<Stf, InnerVm, OuterVm, Da>
where
    Stf::PreState: NativeStorage<Root = Stf::StateRoot>,
    Stf: StateTransitionFunction<InnerVm, OuterVm, Da::Spec>,
    InnerVm: Zkvm,
    OuterVm: Zkvm,
    Da: DaService,
{
    pub async fn initialize<Sm>(
        self,
        stf: &Stf,
        storage_manager: &mut Sm,
    ) -> anyhow::Result<(Stf::StateRoot, Stf::StateRoot)>
    where
        Sm: HierarchicalStorageManager<
            Da::Spec,
            LedgerChangeSet = SchemaBatch,
            LedgerState = DeltaReader,
            StfState = Stf::PreState,
            StfChangeSet = Stf::ChangeSet,
        >,
    {
        let (prev_state_root, genesis_state_root) = match self {
            InitVariant::Initialized(prev_state_root) => {
                let (prover_storage, _ledger_state) = storage_manager.create_bootstrap_state()?;
                let genesis_state_root = prover_storage.get_root_hash(SlotNumber::GENESIS)?;

                (prev_state_root, genesis_state_root)
            }
            InitVariant::Genesis {
                block,
                genesis_params: params,
            } => {
                let genesis_state_root = initialize_state::<Stf, InnerVm, OuterVm, Da, Sm>(
                    stf,
                    storage_manager,
                    block,
                    params,
                )
                .await?;
                (genesis_state_root.clone(), genesis_state_root)
            }
        };

        Ok((prev_state_root, genesis_state_root))
    }
}

fn get_da_polling_interval_ms(da_config: &MockDaConfig) -> u64 {
    match da_config.block_producing {
        BlockProducingConfig::Periodic { block_time_ms } =>
        // 10 times per block, but 10 ms in the worst case
        {
            block_time_ms.checked_div(5).unwrap_or(10)
        }
        _ => 150,
    }
}

pub fn rollup_config_with_da<Da: DaService<Config = MockDaConfig>>(
    path: &std::path::Path,
    da_config: MockDaConfig,
    sequencer_address: <Da::Spec as DaSpec>::Address,
    aggregated_proof_block_jump: usize,
) -> RollupConfig<MockAddress, Da> {
    RollupConfig {
        storage: StorageConfig {
            path: path.to_path_buf(),
        },
        runner: RunnerConfig {
            genesis_height: 0,
            da_polling_interval_ms: get_da_polling_interval_ms(&da_config),
            http_config: HttpServerConfig::localhost_on_free_port(),
            concurrent_sync_tasks: Some(1),
        },
        da: da_config,
        proof_manager: ProofManagerConfig {
            aggregated_proof_block_jump: NonZero::new(aggregated_proof_block_jump).unwrap(),
            prover_address: MockAddress::new([0u8; 32]),
            max_number_of_transitions_in_db: NonZero::new(30).unwrap(),
            max_number_of_transitions_in_memory: NonZero::new(20).unwrap(),
        },
        sequencer: SequencerConfig {
            automatic_batch_production: true,
            max_allowed_blocks_behind: 5,
            // Set ttl to zero to disable for testing. This prevents nondeterminism.
            dropped_tx_ttl_secs: 0,
            admin_addresses: vec![],
            da_address: sequencer_address,
            rollup_address: MockAddress::new([0u8; 32]),
            sequencer_kind_config: SequencerKindConfig::Standard(StdSequencerConfig {
                mempool_max_txs_count: None,
                max_batch_size_bytes: None,
            }),
        },
        monitoring: MonitoringConfig::standard(),
    }
}

fn rollup_config(
    da_service: &MockDaService,
    path: &std::path::Path,
    aggregated_proof_block_jump: usize,
) -> RollupConfig<MockAddress, MockDaService> {
    rollup_config_with_da::<MockDaService>(
        path,
        MockDaConfig::instant_with_sender(da_service.sequencer_address()),
        da_service.sequencer_address(),
        aggregated_proof_block_jump,
    )
}
