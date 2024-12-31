use std::net::SocketAddr;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;

use axum::body::HttpBody;
use axum::extract::Request;
use axum::ServiceExt;
use jsonrpsee::RpcModule;
use sov_db::ledger_db::{LedgerDb, SlotCommit};
use sov_db::schema::{DeltaReader, SchemaBatch};
use sov_metrics::RunnerMetrics;
use sov_rollup_interface::da::{BlobReaderTrait, BlockHeaderTrait, DaSpec};
use sov_rollup_interface::node::da::{DaService, SlotData};
use sov_rollup_interface::node::ledger_api::LedgerStateProvider;
use sov_rollup_interface::node::{
    future_or_shutdown, DaSyncState, FutureOrShutdownOutput, SyncStatus,
};
use sov_rollup_interface::stf::{
    ExecutionContext, ProofOutcome, ProofReceipt, ProofReceiptContents, StateTransitionFunction,
    StoredEvent,
};
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_rollup_interface::zk::aggregated_proof::SerializedAggregatedProof;
use sov_rollup_interface::zk::{StateTransitionWitness, Zkvm};
use sov_rollup_interface::{ProvableHeightTracker, StateUpdateInfo};
use tokio::sync::watch;
use tower_http::normalize_path::NormalizePathLayer;
use tower_layer::Layer;
use tracing::{debug, info, trace};

use crate::da_pre_fetcher::FinalizedBlocksBulkFetcher;
use crate::processes::{new_stf_info_channel, Receiver};
use crate::state_manager::StateManager;
use crate::{MonitoringConfig, ProofManagerConfig, RunnerConfig};

type GenesisParams<ST, InnerVm, OuterVm, Da> =
    <ST as StateTransitionFunction<InnerVm, OuterVm, Da>>::GenesisParams;

type NextDaHeightToProcess = u64;

/// Combines `DaService` with `StateTransitionFunction` and "runs" the rollup.
#[allow(clippy::type_complexity)]
pub struct StateTransitionRunner<Stf, Sm, Da, InnerVm, OuterVm>
where
    Da: DaService,
    InnerVm: Zkvm,
    OuterVm: Zkvm,
    Sm: HierarchicalStorageManager<Da::Spec>,
    Stf: StateTransitionFunction<
        InnerVm,
        OuterVm,
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
    st_info_receiver: Option<Receiver<Stf::StateRoot, Stf::Witness, Da::Spec>>,
    sync_state: Arc<DaSyncState>,
    sync_fetcher: FinalizedBlocksBulkFetcher<Da>,
    shutdown_receiver: watch::Receiver<()>,
    secondary_shutdown_sender: watch::Sender<()>,
    background_handles: Vec<tokio::task::JoinHandle<anyhow::Result<()>>>,
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
pub async fn initialize_state<Stf, InnerVm, OuterVm, Da, Sm>(
    stf: &Stf,
    storage_manager: &mut Sm,
    ledger_db: &LedgerDb,
    genesis_block: Da::FilteredBlock,
    genesis_params: GenesisParams<Stf, InnerVm, OuterVm, Da::Spec>,
) -> anyhow::Result<Stf::StateRoot>
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

    let (genesis_state_root, initialized_storage) = stf.init_chain(
        &block_header,
        &genesis_block.validity_condition(),
        stf_state,
        genesis_params,
    );

    let data_to_commit: SlotCommit<_, Stf::BatchReceiptContents, Stf::TxReceiptContents> =
        SlotCommit::new(genesis_block);
    let mut ledger_change_set =
        ledger_db.materialize_slot(data_to_commit, genesis_state_root.as_ref())?;

    let finalized_slot_changes = ledger_db.materialize_latest_finalize_slot(0)?;

    ledger_change_set.merge(finalized_slot_changes);
    storage_manager.save_change_set(&block_header, initialized_storage, ledger_change_set)?;
    storage_manager.finalize(&block_header)?;

    Ok(genesis_state_root)
}

impl<Stf, Sm, Da, InnerVm, OuterVm> StateTransitionRunner<Stf, Sm, Da, InnerVm, OuterVm>
where
    Da: DaService<Error = anyhow::Error>,
    InnerVm: Zkvm,
    OuterVm: Zkvm,
    Sm: HierarchicalStorageManager<
        Da::Spec,
        LedgerChangeSet = SchemaBatch,
        LedgerState = DeltaReader,
    >,
    Sm::StfState: Clone,
    Stf: StateTransitionFunction<
        InnerVm,
        OuterVm,
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
        pm_config: Option<ProofManagerConfig<Stf::Address>>,
        da_service: Arc<Da>,
        ledger_db: LedgerDb,
        stf: Stf,
        storage_manager: Sm,
        state_update_channel: watch::Sender<StateUpdateInfo<Sm::StfState>>,
        prev_state_root: Stf::StateRoot,
        sync_status_sender: watch::Sender<SyncStatus>,
        state_height_tracker: Box<dyn ProvableHeightTracker>,
        shutdown_receiver: watch::Receiver<()>,
        monitoring_config: MonitoringConfig,
    ) -> anyhow::Result<Self> {
        error_if_tokio_runtime_is_not_multi_threaded()?;

        let rpc_config = &runner_config.rpc_config;
        let axum_config = &runner_config.axum_config;

        let listen_address_rpc =
            SocketAddr::new(rpc_config.bind_host.parse()?, rpc_config.bind_port);
        let listen_address_axum =
            SocketAddr::new(axum_config.bind_host.parse()?, axum_config.bind_port);

        let next_item_numbers = ledger_db.get_next_items_numbers()?;
        let last_slot_processed_before_shutdown = next_item_numbers.rollup_height.saturating_sub(1);

        let da_height_processed =
            runner_config.genesis_height + last_slot_processed_before_shutdown;

        let first_unprocessed_height_at_startup = da_height_processed + 1;
        debug!(
            %last_slot_processed_before_shutdown,
            %runner_config.genesis_height,
            %first_unprocessed_height_at_startup,
            proof_manager_config = ?pm_config,
            "Initializing StfRunner");

        let (st_info_sender, st_info_receiver) = if let Some(config) = pm_config {
            let channel = new_stf_info_channel(
                ledger_db.clone(),
                config.max_number_of_transitions_in_memory,
                config.max_number_of_transitions_in_db,
            )
            .await?;

            (Some(channel.0), Some(channel.1))
        } else {
            (None, None)
        };

        let state_manager = StateManager::new(
            storage_manager,
            ledger_db,
            prev_state_root,
            state_update_channel,
            st_info_sender,
            state_height_tracker,
        )?;

        let (sync_fetcher, fetcher_background_handle) = FinalizedBlocksBulkFetcher::new(
            da_service.clone(),
            first_unprocessed_height_at_startup,
            runner_config.get_concurrent_sync_tasks(),
            shutdown_receiver.clone(),
        )
        .await?;

        let (secondary_shutdown_sender, mut secondary_shutdown_receiver) = watch::channel(());
        secondary_shutdown_receiver.mark_unchanged();

        let monitoring_handle: tokio::task::JoinHandle<anyhow::Result<()>> =
            tokio::spawn(async move {
                sov_metrics::init_metrics_tracker(
                    secondary_shutdown_receiver.clone(),
                    &monitoring_config,
                )
                .await?;
                Ok(())
            });

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
            st_info_receiver,
            sync_fetcher,
            shutdown_receiver,
            secondary_shutdown_sender,
            background_handles: vec![fetcher_background_handle, monitoring_handle],
        })
    }

    /// Subscribes to this runner's [`StateUpdateInfo`] channel, if enabled.
    ///
    /// Only one [`Receiver`] is allowed, and subsequent calls to this method
    /// will return [`None`].
    pub fn take_st_info_receiver(
        &mut self,
    ) -> Option<Receiver<Stf::StateRoot, Stf::Witness, Da::Spec>> {
        self.st_info_receiver.take()
    }

    /// Returns the [`DaSyncState`] of the node.
    pub fn da_sync_state(&self) -> Arc<DaSyncState> {
        self.sync_state.clone()
    }

    /// Starts an RPC server with provided RPC methods and returns [`SocketAddr`] it is bind to.
    ///  # Arguments:
    ///   * methods: [`RpcModule`] with all RPC methods.
    pub async fn start_rpc_server(&mut self, methods: RpcModule<()>) -> anyhow::Result<SocketAddr> {
        let server = jsonrpsee::server::ServerBuilder::default()
            .build([self.listen_address_rpc].as_ref())
            .await?;
        let rpc_address = server.local_addr()?;

        let mut shutdown_receiver = self.secondary_shutdown_sender.subscribe();

        self.background_handles.push(tokio::spawn(async move {
            info!(%rpc_address, "Starting RPC server");
            let server_handle = server.start(methods);

            shutdown_receiver.changed().await.ok();
            info!("Shutting down RPC server...");
            server_handle.stop().map_err(|e| anyhow::anyhow!(e))?;
            // Wait till the RPC server actually stopped,
            // So when this task is completed,
            // it is safe to assume that the RPC server is running no more.
            server_handle.stopped().await;
            debug!("RPC server stopped");

            Ok(())
        }));

        Ok(rpc_address)
    }

    /// Starts an Axum server with the provided router.
    pub async fn start_axum_server(
        &mut self,
        router: axum::Router<()>,
    ) -> anyhow::Result<SocketAddr> {
        let listener = tokio::net::TcpListener::bind(self.listen_address_axum).await?;
        let rest_address = listener.local_addr()?;

        let mut shutdown_receiver = self.secondary_shutdown_sender.subscribe();

        self.background_handles.push(tokio::spawn(async move {
            info!(%rest_address, "Starting REST API server");
            let router = router.layer(axum::middleware::from_fn(measure_time));
            let router = NormalizePathLayer::trim_trailing_slash().layer(router);

            axum::serve(listener, ServiceExt::<Request>::into_make_service(router))
                .with_graceful_shutdown(async move {
                    shutdown_receiver.changed().await.ok();
                })
                .await
                .map_err(|e| anyhow::anyhow!(e))
        }));

        Ok(rest_address)
    }

    /// Spawn a [`tokio::task`] that updates the sync status every `polling_interval`.
    fn spawn_sync_status_updater(
        &self,
        polling_interval: Duration,
        shutdown_receiver: watch::Receiver<()>,
    ) -> tokio::task::JoinHandle<()> {
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
                match future_or_shutdown(da_service.get_head_block_header(), &shutdown_receiver)
                    .await
                {
                    FutureOrShutdownOutput::Shutdown => break,
                    FutureOrShutdownOutput::Output(Err(_)) => continue,
                    FutureOrShutdownOutput::Output(Ok(header)) => {
                        let target_da_height = header.height();
                        sync_state.update_target(target_da_height);
                        if let SyncStatus::Syncing {
                            synced_da_height,
                            target_da_height,
                        } = sync_state.status()
                        {
                            let distance = target_da_height - synced_da_height;
                            if distance > 1 {
                                info!(synced_da_height, target_da_height, "Sync in progress");
                            } else {
                                trace!(synced_da_height, target_da_height, "Sync in progress");
                            }
                        }
                        future_or_shutdown(interval.tick(), &shutdown_receiver).await;
                    }
                }
            }
        })
    }

    /// Runs the rollup.
    pub async fn run_in_process(&mut self) -> anyhow::Result<()> {
        self.state_manager.startup().await?;
        let mut next_da_height = self.first_unprocessed_height_at_startup;
        let target_da_height = self.da_service.get_head_block_header().await?.height();
        self.sync_state.update_target(target_da_height);

        let status_updater_handle = self.spawn_sync_status_updater(
            Duration::from_millis(self.da_polling_interval_ms),
            self.shutdown_receiver.clone(),
        );

        loop {
            let shutdown_receiver = self.shutdown_receiver.clone();
            match future_or_shutdown(self.process_next_slot(next_da_height), &shutdown_receiver)
                .await
            {
                FutureOrShutdownOutput::Shutdown => break,
                FutureOrShutdownOutput::Output(slot_result) => {
                    next_da_height = slot_result?;
                }
            }
        }
        info!("Runner main loop is completed, keep shutting down...");
        status_updater_handle.await?;
        debug!("Status updater stopped, sending secondary shutdown for runner");
        self.secondary_shutdown_sender.send(())?;
        let background_handles = std::mem::take(&mut self.background_handles);
        for handle in background_handles {
            let _ = handle.await?;
        }
        Ok(())
    }

    #[tracing::instrument(skip(self))]
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
            .proof_blobs
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
        // New influxdb metrics
        sov_metrics::track_metrics(|metrics| {
            let synced_da_height = self
                .sync_state
                .synced_da_height
                .load(std::sync::atomic::Ordering::Acquire);
            let target_da_height = self
                .sync_state
                .target_da_height
                .load(std::sync::atomic::Ordering::Acquire);
            let point = RunnerMetrics {
                sync_distance: target_da_height as i64 - synced_da_height as i64,
                da_height: next_da_height,
                get_block_time,
                batches_processed: batch_count,
                batch_bytes_processed,
                proofs_processed: proof_blobs_processed as u64,
                proof_bytes_processed,
                transactions_processed: transaction_count as u64,
                process_slot_time: loop_start.elapsed(),
                apply_slot_time,
                stf_transition_time: stf_execution_start.elapsed(),
                extract_blobs_time: da_extraction_time,
                extraction_proof_time: get_relevant_proofs_time,
            };
            metrics.track_runner_metrics(point);
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
    ) -> Vec<SerializedAggregatedProof> {
        let mut aggregated_proofs: Vec<SerializedAggregatedProof> = Vec::new();
        for receipt in receipts {
            match receipt.outcome {
                ProofOutcome::Valid(ProofReceiptContents::AggregateProof(
                    _public_data,
                    raw_proof,
                )) => {
                    aggregated_proofs.push(raw_proof);
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

/// Creates a new [`StateUpdateInfo`] with some storage state and chain data
/// queried from [`LedgerDb`].
pub async fn query_state_update_info<S>(
    ledger_db: &LedgerDb,
    storage: S,
) -> anyhow::Result<StateUpdateInfo<S>> {
    let rollup_height = ledger_db
        .get_head_rollup_height()
        .await?
        .expect("The rollup height should always be available");
    let next_event_number = ledger_db
        .get_latest_event_number()
        .await?
        .map(|x| x + 1)
        .unwrap_or_default();
    let latest_finalized_rollup_height = ledger_db.get_latest_finalized_rollup_height().await?;

    Ok(StateUpdateInfo {
        storage,
        next_event_number,
        rollup_height,
        latest_finalized_rollup_height,
    })
}

fn error_if_tokio_runtime_is_not_multi_threaded() -> anyhow::Result<()> {
    use tokio::runtime::{Handle, RuntimeFlavor};

    match Handle::current().runtime_flavor() {
        RuntimeFlavor::CurrentThread => Err(anyhow::anyhow!("A multi-threaded Tokio runtime is required to run the rollup node. Check your Tokio configuration. If you're testing node functionality, make sure your test uses `#[tokio::test(flavor = \"multi_thread\")]` or an equivalent configuration. Aborting.")),
        _ => Ok(())
    }
}

async fn measure_time(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> impl axum::response::IntoResponse {
    let method = req.method().clone();
    let uri = req.uri().clone();

    let start = std::time::Instant::now();

    let response = next.run(req).await;
    let duration = start.elapsed();

    let body = response.body();
    let status = response.status();
    let size_hint = body.size_hint();
    let exact_or_lower = size_hint.exact().unwrap_or_else(|| size_hint.lower());

    sov_metrics::track_metrics(|tracker| {
        let point = sov_metrics::HttpMetrics {
            request_method: method,
            request_uri: uri,
            response_status: status,
            response_body_size: exact_or_lower,
            handler_processing_time: duration,
        };
        tracker.track_http_request(point);
    });

    response
}
