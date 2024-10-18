use std::net::SocketAddr;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::Request;
use axum::ServiceExt;
use jsonrpsee::RpcModule;
use sov_db::ledger_db::{LedgerDb, SlotCommit};
use sov_db::schema::{DeltaReader, SchemaBatch};
use sov_rollup_interface::da::{BlobReaderTrait, BlockHeaderTrait, DaSpec};
use sov_rollup_interface::node::da::{DaService, SlotData};
use sov_rollup_interface::node::ledger_api::{LedgerStateProvider, QueryMode};
use sov_rollup_interface::node::{DaSyncState, SyncStatus};
use sov_rollup_interface::stf::{
    ExecutionContext, ProofOutcome, ProofReceipt, ProofReceiptContents, StateTransitionFunction,
    StoredEvent,
};
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_rollup_interface::zk::aggregated_proof::AggregatedProof;
use sov_rollup_interface::zk::{StateTransitionWitness, Zkvm, ZkvmGuest, ZkvmHost};
use tokio::sync::watch;
use tower_http::normalize_path::NormalizePathLayer;
use tower_layer::Layer;
use tracing::{debug, error, info};

use crate::da_pre_fetcher::FinalizedBlocksBulkFetcher;
use crate::processes::{RawGenesisStateRoot, Sender};
use crate::state_manager::StateManager;
use crate::RunnerConfig;

type GenesisParams<ST, InnerVm, OuterVm, Da> =
    <ST as StateTransitionFunction<InnerVm, OuterVm, Da>>::GenesisParams;

type Verifier<Host> = <<Host as ZkvmHost>::Guest as ZkvmGuest>::Verifier;

type NextDaHeightToProcess = u64;

/// Combines `DaService` with `StateTransitionFunction` and "runs" the rollup.
#[allow(clippy::type_complexity)]
pub struct StateTransitionRunner<Stf, Sm, Da, InnerVm, OuterVm>
where
    Da: DaService,
    InnerVm: ZkvmHost,
    OuterVm: ZkvmHost,
    Sm: HierarchicalStorageManager<Da::Spec>,
    Stf: StateTransitionFunction<
        Verifier<InnerVm>,
        Verifier<OuterVm>,
        Da::Spec,
        Condition = <Da::Spec as DaSpec>::ValidityCondition,
    >,
{
    first_unprocessed_height_at_startup: u64,
    da_polling_interval_ms: u64,
    da_service: Arc<Da>,
    stf: Stf,
    state_manager: StateManager<Stf::StateRoot, Stf::Witness, Sm, Da>,
    listen_address_rpc: SocketAddr,
    listen_address_axum: SocketAddr,
    sync_state: Arc<DaSyncState>,
    sync_fetcher: FinalizedBlocksBulkFetcher<Da>,
}

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

struct DiscardEvents;
impl TryFrom<(u64, StoredEvent)> for DiscardEvents {
    type Error = anyhow::Error;

    fn try_from(_value: (u64, StoredEvent)) -> Result<Self, Self::Error> {
        Ok(Self)
    }
}

/// Initializes rollup genesis.
/// Gets proper DA block and finalizes storage.
/// Returns root hashes.
async fn initialize_state<Stf, InnerVm, OuterVm, Da, Sm>(
    stf: &Stf,
    storage_manager: &mut Sm,
    ledger_db: &LedgerDb,
    genesis_block: Da::FilteredBlock,
    genesis_params: GenesisParams<Stf, InnerVm, OuterVm, Da::Spec>,
) -> anyhow::Result<(Stf::StateRoot, RawGenesisStateRoot)>
where
    Stf: StateTransitionFunction<InnerVm, OuterVm, Da::Spec>,
    InnerVm: Zkvm,
    OuterVm: Zkvm,
    Da: DaService,
    Sm: HierarchicalStorageManager<
        Da::Spec,
        LedgerChangeSet = SchemaBatch,
        LedgerState = DeltaReader,
        StfState = Stf::PreState,
        StfChangeSet = Stf::ChangeSet,
    >,
{
    let block_header = genesis_block.header().clone();
    info!(
        header = %block_header.display(),
        "No history detected. Initializing chain on the block header..."
    );
    // Ledger state is not used, as we know it should be empty
    let (stf_state, _ledger_state) = storage_manager.create_state_for(&block_header)?;

    let (genesis_state_root, initialized_storage) = stf.init_chain(stf_state, genesis_params);
    let data_to_commit: SlotCommit<
        _,
        Stf::BatchReceiptContents,
        Stf::TxReceiptContents,
        Stf::GasPrice,
    > = SlotCommit::new(genesis_block);
    let mut ledger_change_set =
        ledger_db.materialize_slot(data_to_commit, genesis_state_root.as_ref())?;

    let finalized_slot_changes = ledger_db.materialize_latest_finalize_slot(0)?;

    ledger_change_set.merge(finalized_slot_changes);
    storage_manager.save_change_set(&block_header, initialized_storage, ledger_change_set)?;
    storage_manager.finalize(&block_header)?;
    let raw_genesis_state_root = RawGenesisStateRoot(genesis_state_root.as_ref().to_vec());
    Ok((genesis_state_root, raw_genesis_state_root))
}

impl<
        Stf: StateTransitionFunction<InnerVm, OuterVm, Da::Spec>,
        InnerVm: Zkvm,
        OuterVm: Zkvm,
        Da: DaService,
    > InitVariant<Stf, InnerVm, OuterVm, Da>
{
    /// Initializes the rollup state and calculates initial state roots for the rollup.
    pub async fn initialize<Sm>(
        self,
        ledger_db: &mut LedgerDb,
        stf: &Stf,
        storage_manager: &mut Sm,
    ) -> anyhow::Result<(Stf::StateRoot, RawGenesisStateRoot)>
    where
        Sm: HierarchicalStorageManager<
            Da::Spec,
            LedgerChangeSet = SchemaBatch,
            LedgerState = DeltaReader,
            StfState = Stf::PreState,
            StfChangeSet = Stf::ChangeSet,
        >,
    {
        let (prev_state_root, raw_genesis_state_root) = match self {
            InitVariant::Initialized(prev_state_root) => {
                //
                debug!("Chain is already initialized; skipping initialization");
                // Since we're just getting the state root, we don't care about fetching the events.
                // QueryModeCompact prevents us from actually fetching them, but we still need to provide a value for
                // the event generic, so we use a dummy type.
                let raw_genesis_state_root = ledger_db
                    .get_slot_by_number::<Stf::BatchReceiptContents, Stf::TxReceiptContents, DiscardEvents>(
                        0,
                        QueryMode::Compact,
                    )
                    .await?
                    .expect("Rollup was already initialized. Slot 0 should exist")
                    .state_root;
                (prev_state_root, RawGenesisStateRoot(raw_genesis_state_root))
            }
            InitVariant::Genesis {
                block,
                genesis_params: params,
            } => {
                initialize_state::<Stf, InnerVm, OuterVm, Da, Sm>(
                    stf,
                    storage_manager,
                    ledger_db,
                    block,
                    params,
                )
                .await?
            }
        };

        info!(
            genesis_state_root = hex::encode(&raw_genesis_state_root.0),
            "Chain initialization is done"
        );

        Ok((prev_state_root, raw_genesis_state_root))
    }
}

impl<Stf, Sm, Da, InnerVm, OuterVm> StateTransitionRunner<Stf, Sm, Da, InnerVm, OuterVm>
where
    Da: DaService<Error = anyhow::Error> + Clone,
    InnerVm: ZkvmHost,
    OuterVm: ZkvmHost,
    Sm: HierarchicalStorageManager<
        Da::Spec,
        LedgerChangeSet = SchemaBatch,
        LedgerState = DeltaReader,
    >,
    Sm::StfState: Clone,
    Stf: StateTransitionFunction<
        <InnerVm::Guest as ZkvmGuest>::Verifier,
        <OuterVm::Guest as ZkvmGuest>::Verifier,
        Da::Spec,
        Condition = <Da::Spec as DaSpec>::ValidityCondition,
        PreState = Sm::StfState,
        ChangeSet = Sm::StfChangeSet,
    >,
{
    /// Creates a new [`StateTransitionRunner`].
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]

    pub async fn new(
        runner_config: RunnerConfig,
        da_service: Arc<Da>,
        ledger_db: LedgerDb,
        stf: Stf,
        storage_manager: Sm,
        api_storage_sender: watch::Sender<Sm::StfState>,
        prev_state_root: Stf::StateRoot,
        st_info_sender: Option<Sender<Stf::StateRoot, Stf::Witness, Da::Spec>>,
        sync_status_sender: watch::Sender<SyncStatus>,
    ) -> anyhow::Result<Self> {
        let rpc_config = &runner_config.rpc_config;
        let axum_config = &runner_config.axum_config;

        let listen_address_rpc =
            SocketAddr::new(rpc_config.bind_host.parse()?, rpc_config.bind_port);
        let listen_address_axum =
            SocketAddr::new(axum_config.bind_host.parse()?, axum_config.bind_port);

        // Start the main rollup loop
        let next_item_numbers = ledger_db.get_next_items_numbers()?;
        let last_slot_processed_before_shutdown = next_item_numbers.slot_number.saturating_sub(1);

        let da_height_processed =
            runner_config.genesis_height + last_slot_processed_before_shutdown;

        let first_unprocessed_height_at_startup = da_height_processed + 1;
        debug!(%last_slot_processed_before_shutdown, %runner_config.genesis_height, %first_unprocessed_height_at_startup, "Initializing StfRunner");

        let state_manager = StateManager::new(
            storage_manager,
            ledger_db,
            prev_state_root,
            api_storage_sender,
            st_info_sender,
        );

        let sync_fetcher = FinalizedBlocksBulkFetcher::new(
            da_service.clone(),
            first_unprocessed_height_at_startup,
            runner_config.get_concurrent_sync_tasks(),
        )
        .await?;

        Ok(Self {
            first_unprocessed_height_at_startup,
            da_polling_interval_ms: runner_config.da_polling_interval_ms,
            da_service: da_service.clone(),
            stf,
            state_manager,
            listen_address_rpc,
            listen_address_axum,
            sync_state: Arc::new(DaSyncState {
                synced_da_height: AtomicU64::new(da_height_processed),
                target_da_height: AtomicU64::new(u64::MAX),
                sync_status_sender,
            }),
            sync_fetcher,
        })
    }

    /// Returns the [`DaSyncState`] of the node.
    pub fn da_sync_state(&self) -> Arc<DaSyncState> {
        self.sync_state.clone()
    }

    /// Starts an RPC server with provided RPC methods and returns [`SocketAddr`] it is bind to.
    ///  # Arguments:
    ///   * methods: [`RpcModule`] with all RPC methods.
    pub async fn start_rpc_server(&self, methods: RpcModule<()>) -> anyhow::Result<SocketAddr> {
        let server = jsonrpsee::server::ServerBuilder::default()
            .build([self.listen_address_rpc].as_ref())
            .await?;
        let rpc_address = server.local_addr()?;

        let _handle = tokio::spawn(async move {
            info!(%rpc_address, "Starting RPC server");
            let _server_handle = server.start(methods);

            futures::future::pending::<()>().await;
        });

        Ok(rpc_address)
    }

    /// Starts an Axum server with the provided router.
    pub async fn start_axum_server(&self, router: axum::Router<()>) -> anyhow::Result<SocketAddr> {
        let listener = tokio::net::TcpListener::bind(self.listen_address_axum).await?;
        let rest_address = listener.local_addr()?;

        tokio::spawn(async move {
            info!(%rest_address, "Starting REST API server");
            let router = NormalizePathLayer::trim_trailing_slash().layer(router);

            axum::serve(listener, ServiceExt::<Request>::into_make_service(router))
                .await
                .unwrap();
        });

        Ok(rest_address)
    }

    /// Spawn a [`tokio::task`] that updates the sync status every `polling_interval`.
    pub fn spawn_sync_status_updater(&self, polling_interval: Duration) {
        let sync_state = self.sync_state.clone();
        let da_service = self.da_service.clone();

        tokio::task::spawn(async move {
            let mut interval = tokio::time::interval(polling_interval);
            debug!(
                interval_ms = interval.period().as_millis(),
                "Interval for polling sync DA height"
            );
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await; // Tick the interval once because it starts at 0ms.

            loop {
                let target_da_height = match da_service.get_head_block_header().await {
                    Ok(header) => header.height(),
                    Err(error) => {
                        error!(
                            ?error,
                            "Failed to get the DA head block header during sync status update; will retry in ~{}ms",
                            polling_interval.as_millis()
                        );
                        continue;
                    }
                };

                sync_state.update_target(target_da_height);

                if let SyncStatus::Syncing {
                    synced_da_height,
                    target_da_height,
                } = sync_state.status()
                {
                    info!(synced_da_height, target_da_height, "Sync in progress");
                }

                interval.tick().await;
            }
        });
    }

    /// Runs the rollup.
    pub async fn run_in_process(&mut self) -> anyhow::Result<()> {
        let mut next_da_height = self.first_unprocessed_height_at_startup;
        let target_da_height = self.da_service.get_head_block_header().await?.height();
        self.sync_state.update_target(target_da_height);

        self.spawn_sync_status_updater(Duration::from_millis(self.da_polling_interval_ms));

        loop {
            next_da_height = self.process_next_slot(next_da_height).await?;
        }
    }

    async fn process_next_slot(
        &mut self,
        mut next_da_height: NextDaHeightToProcess,
    ) -> anyhow::Result<NextDaHeightToProcess> {
        let loop_start = std::time::Instant::now();
        let prev_state_root = self.get_state_root().clone();
        debug!(
            next_da_height,
            current_state_root = hex::encode(prev_state_root.as_ref()),
            "Requesting DA block"
        );
        sov_metrics::update_metrics(|metrics| {
            metrics.current_da_height.set(next_da_height as i64);
        });

        let mut transaction_count = 0;
        let mut batch_count = 0;
        let get_block_start = std::time::Instant::now();
        let filtered_block = self.sync_fetcher.get_block_at(next_da_height).await?;
        let get_block_time = get_block_start.elapsed();
        debug!(header = %filtered_block.header().display(), request_time = ?get_block_start.elapsed(), "Fetched block header");

        let (stf_pre_state, filtered_block) = self
            .state_manager
            .prepare_storage(filtered_block, &self.da_service)
            .await?;

        let filtered_block_header = filtered_block.header().clone();
        if next_da_height != filtered_block_header.height() {
            debug!(
                existing_next_da_height = next_da_height,
                new_next_da_height = filtered_block_header.height(),
                "Updating next_da_height after storage_manager "
            );
            next_da_height = filtered_block_header.height();
        }

        // STF execution
        let stf_execution_start = std::time::Instant::now();
        let mut relevant_blobs = self.da_service.extract_relevant_blobs(&filtered_block);
        let batch_blobs = &mut relevant_blobs.batch_blobs;
        let proof_blobs = &relevant_blobs.proof_blobs;
        debug!(
            batch_blobs_count = batch_blobs.len(),
            next_da_height,
            current_state_root = hex::encode(prev_state_root.as_ref()),
            batch_blobs = ?batch_blobs
                .iter()
                .map(|b| format!(
                    "sequencer={} blob_hash=0x{}",
                    b.sender(),
                    hex::encode(b.hash())
                ))
                .collect::<Vec<_>>(),
            proof_blobs = ?proof_blobs
                .iter()
                .map(|b| format!(
                    "sequencer={} blob_hash=0x{}, len={}",
                    b.sender(),
                    hex::encode(b.hash()),
                    b.total_len()
                ))
                .collect::<Vec<_>>(),
            "Extracted relevant blobs"
        );
        let da_extraction_time = stf_execution_start.elapsed();

        let apply_slot_start = std::time::Instant::now();
        let slot_result = self.stf.apply_slot(
            self.state_manager.get_state_root(),
            stf_pre_state,
            Default::default(),
            &filtered_block_header,
            &filtered_block.validity_condition(),
            relevant_blobs.as_iters(),
            ExecutionContext::Node,
        );
        let apply_slot_time = apply_slot_start.elapsed();

        // --- Before destructuring the receipt, extract some data for metrics ---
        let batch_bytes_processed: u64 = relevant_blobs
            .batch_blobs
            .iter()
            .map(|b| b.verified_data().len() as u64)
            .sum();
        let proof_bytes_processed: u64 = relevant_blobs
            .batch_blobs
            .iter()
            .map(|b| b.verified_data().len() as u64)
            .sum();
        let proof_blobs_processed = slot_result.proof_receipts.len();
        // --- End metric extraction ---

        let get_relevant_proofs_start = std::time::Instant::now();
        // Get merkle proofs for the relevant blobs
        let relevant_proofs = self
            .da_service
            .get_extraction_proof(&filtered_block, &relevant_blobs)
            .await;
        let get_relevant_proofs_time = get_relevant_proofs_start.elapsed();
        // Handling executed data
        let mut data_to_commit = SlotCommit::new(filtered_block);
        for receipt in slot_result.batch_receipts {
            batch_count += 1;
            transaction_count += receipt.tx_receipts.len();
            data_to_commit.add_batch(receipt);
        }

        let transition_data: StateTransitionWitness<Stf::StateRoot, Stf::Witness, Da::Spec> =
            StateTransitionWitness {
                initial_state_root: self.get_state_root().clone(),
                final_state_root: slot_result.state_root.clone(),
                da_block_header: filtered_block_header.clone(),
                relevant_proofs,
                relevant_blobs,
                witness: slot_result.witness,
            };

        let aggregated_proofs =
            Self::collect_aggregated_proofs(slot_result.proof_receipts.into_iter());

        // Processing finalized headers.
        let last_finalized = self.da_service.get_last_finalized_block_header().await?;
        debug!(header = %last_finalized.display(), "Got last finalized header");
        let last_finalized_height = last_finalized.height();

        self.state_manager
            .process_stf_changes(
                last_finalized_height,
                slot_result.change_set,
                transition_data,
                data_to_commit,
                aggregated_proofs,
            )
            .await?;

        // Updating counters and metrics
        self.sync_state.update_synced(next_da_height);
        debug!(
            height = next_da_height,
            prev_state_root = hex::encode(prev_state_root.as_ref()),
            new_state_root = hex::encode(self.get_state_root().as_ref()),
            time = ?loop_start.elapsed(),
            "Execution of block is completed"
        );
        sov_metrics::update_metrics(|metrics| {
            metrics.da_blocks_processed.inc();
            metrics.rollup_batches_processed.inc_by(batch_count);
            metrics.batch_bytes_processed.inc_by(batch_bytes_processed);
            metrics.proof_bytes_processed.inc_by(proof_bytes_processed);
            metrics
                .proof_blobs_processed
                .inc_by(proof_blobs_processed as _);
            metrics.rollup_txns_processed.inc_by(transaction_count as _);
            let synced_da_height = self
                .sync_state
                .synced_da_height
                .load(std::sync::atomic::Ordering::Acquire);
            let target_da_height = self
                .sync_state
                .target_da_height
                .load(std::sync::atomic::Ordering::Acquire);

            let distance = target_da_height as i64 - synced_da_height as i64;
            metrics.sync_distance.set(distance);

            metrics
                .process_slot_sec
                .observe(loop_start.elapsed().as_secs_f64());
            metrics
                .stf_transition_sec
                .observe(stf_execution_start.elapsed().as_secs_f64());
            metrics.get_block_sec.observe(get_block_time.as_secs_f64());

            metrics
                .process_slot_ms_by_slot
                .set(loop_start.elapsed().as_millis() as i64);
            metrics
                .stf_transition_with_commit_ms_by_slot
                .set(apply_slot_start.elapsed().as_millis() as i64);
            metrics
                .apply_slot_ms_by_slot
                .set(apply_slot_time.as_millis() as i64);
            metrics
                .extract_blobs_ms_by_slot
                .set(da_extraction_time.as_millis() as i64);
            metrics
                .get_blob_extraction_proof_ms_by_slot
                .set(get_relevant_proofs_time.as_millis() as i64);
            metrics
                .get_block_ms_by_slot
                .set(get_block_time.as_millis() as i64);
        });

        Ok(next_da_height + 1)
    }

    /// Allows reading current state root
    pub fn get_state_root(&self) -> &Stf::StateRoot {
        self.state_manager.get_state_root()
    }

    /// Retrieve a handle for the underlying DA service
    pub fn da_service(&self) -> Arc<Da> {
        self.da_service.clone()
    }

    fn collect_aggregated_proofs(
        receipts: impl Iterator<
            Item = ProofReceipt<Stf::Address, Da::Spec, Stf::StateRoot, Stf::StorageProof>,
        >,
    ) -> Vec<AggregatedProof> {
        let mut aggregated_proofs: Vec<AggregatedProof> = Vec::new();
        for receipt in receipts {
            match receipt.outcome {
                ProofOutcome::Valid(ProofReceiptContents::AggregateProof(
                    public_data,
                    raw_proof,
                )) => {
                    aggregated_proofs.push(AggregatedProof::new(raw_proof, public_data));
                }
                ProofOutcome::Valid(_) => {
                    tracing::info!("Not aggregated proof, probably running in a different mode. Will be fixed in the future.");
                }
                _ => {
                    tracing::error!("Invalid proof outcome, {:?}", receipt.outcome);
                }
            }
        }

        aggregated_proofs
    }
}
