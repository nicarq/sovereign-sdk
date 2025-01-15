mod endpoints;
pub mod logging;
pub mod proof_serializer;
mod telemetry;
mod wallet;

use std::marker::PhantomData;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
pub use endpoints::*;
use sov_db::ledger_db::LedgerDb;
use sov_db::schema::{DeltaReader, SchemaBatch};
use sov_modules_api::capabilities::{HasCapabilities, ProofProcessor};
use sov_modules_api::execution_mode::ExecutionMode;
use sov_modules_api::prelude::axum;
use sov_modules_api::provable_height_tracker::MaximumProvableHeight;
use sov_modules_api::rest::{ApiState, StateUpdateReceiver};
use sov_modules_api::{
    BatchSequencerReceipt, OperatingMode, ProofSerializer, RuntimeEndpoints, RuntimeEventProcessor,
    RuntimeEventResponse, Spec, StateUpdateInfo, SyncStatus, ZkVerifier,
};
use sov_modules_stf_blueprint::{
    GenesisParams, Runtime as RuntimeTrait, StfBlueprint, TxReceiptContents,
};
use sov_rollup_interface::common::SlotNumber;
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::node::DaSyncState;
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_rollup_interface::ProvableHeightTracker;
use sov_sequencer::batch_builders::preferred::PreferredBatchBuilder;
use sov_sequencer::batch_builders::standard::StdBatchBuilder;
use sov_sequencer::batch_builders::BatchBuilder;
use sov_sequencer::{BatchBuilderConfig, SequenceNumberProvider, Sequencer, SequencerSpec};
use sov_state::storage::NativeStorage;
use sov_state::Storage;
use sov_stf_runner::processes::{ProverService, RollupProverConfig, WorkflowProcessManager};
use sov_stf_runner::{
    initialize_state, query_state_update_info, RollupConfig, StateTransitionRunner,
};
use tokio::signal::unix::SignalKind;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tracing::info;
pub use wallet::*;

use crate::RollupBlueprint;

/// This trait defines how to create all the necessary dependencies required by a rollup.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
#[async_trait]
pub trait FullNodeBlueprint<M: ExecutionMode>: RollupBlueprint<M> {
    /// Data Availability service.
    type DaService: DaService<Spec = <Self::Spec as Spec>::Da, Error = anyhow::Error>;

    /// Manager for the native storage lifecycle.
    type StorageManager: HierarchicalStorageManager<
        <Self::Spec as Spec>::Da,
        StfState = <Self::Spec as Spec>::Storage,
        StfChangeSet = <<Self::Spec as Spec>::Storage as Storage>::ChangeSet,
        LedgerState = DeltaReader,
        LedgerChangeSet = SchemaBatch,
    >;

    /// Prover service.
    type ProverService: ProverService<
        StateRoot = <<Self::Spec as Spec>::Storage as Storage>::Root,
        Witness = <<Self::Spec as Spec>::Storage as Storage>::Witness,
        DaService = Self::DaService,
    >;

    /// Serialize proof blob and adds metadata needed for verification.
    type ProofSerializer: ProofSerializer + 'static;

    /// Creates code commitments for the outer zkVM program.
    fn create_outer_code_commitment(
        &self,
    ) -> <<Self::ProverService as ProverService>::Verifier as ZkVerifier>::CodeCommitment;

    /// Creates RPC methods for the rollup.
    async fn create_endpoints(
        &self,
        state_update_receiver: StateUpdateReceiver<<Self::Spec as Spec>::Storage>,
        sync_status_receiver: tokio::sync::watch::Receiver<SyncStatus>,
        ledger_db: &LedgerDb,
        sequencer: &SequencerCreationReceipt<Self::Spec>,
        da_service: &Self::DaService,
        rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
    ) -> anyhow::Result<RuntimeEndpoints>;

    /// Creates GenesisConfig from genesis files.
    #[allow(clippy::type_complexity)]
    fn create_genesis_config(
        &self,
        rt_genesis_paths: &<Self::Runtime as RuntimeTrait<Self::Spec>>::GenesisInput,
        _rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
    ) -> anyhow::Result<GenesisParams<<Self::Runtime as RuntimeTrait<Self::Spec>>::GenesisConfig>>
    {
        let rt_genesis =
            <Self::Runtime as RuntimeTrait<Self::Spec>>::genesis_config(rt_genesis_paths)?;

        Ok(GenesisParams {
            runtime: rt_genesis,
        })
    }

    /// Creates an instance of [`DaService`].
    async fn create_da_service(
        &self,
        rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        shutdown_receiver: watch::Receiver<()>,
    ) -> Self::DaService;

    /// Creates an instance of [`ProverService`].
    async fn create_prover_service(
        &self,
        prover_config: RollupProverConfig<<Self::Spec as Spec>::InnerZkvm>,
        rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        da_service: &Self::DaService,
    ) -> Self::ProverService;

    /// Creates an instance of [`Self::StorageManager`].
    /// Panics if initialization fails.
    fn create_storage_manager(
        &self,
        rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
    ) -> anyhow::Result<Self::StorageManager>;

    /// Instantiates [`FullNodeBlueprint::ProofSerializer`].
    fn create_proof_serializer(
        &self,
        rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        sequence_number_provider: Option<Arc<dyn SequenceNumberProvider>>,
    ) -> anyhow::Result<Self::ProofSerializer>;

    /// Creates an instance of a LedgerDb.
    fn create_ledger_db(
        &self,
        ledger_state: <Self::StorageManager as HierarchicalStorageManager<
            <Self::Spec as Spec>::Da,
        >>::LedgerState,
    ) -> anyhow::Result<LedgerDb> {
        LedgerDb::with_reader(ledger_state)
    }

    /// Creates a new rollup.
    async fn create_new_rollup(
        &self,
        runtime_genesis_paths: &<Self::Runtime as RuntimeTrait<Self::Spec>>::GenesisInput,
        rollup_config: RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        prover_config: Option<RollupProverConfig<<Self::Spec as Spec>::InnerZkvm>>,
    ) -> anyhow::Result<Rollup<Self, M>>
    where
        <Self::Spec as Spec>::Storage: NativeStorage,
    {
        let genesis_params = self.create_genesis_config(runtime_genesis_paths, &rollup_config)?;

        self.create_new_rollup_with_genesis_params(genesis_params, rollup_config, prover_config)
            .await
    }

    /// Creates a new sequencer and provides a [`SequencerCreationReceipt`] with
    /// some information about said sequencer.
    async fn create_sequencer(
        &self,
        state_update_receiver: watch::Receiver<StateUpdateInfo<<Self::Spec as Spec>::Storage>>,
        da_sync_state: Arc<DaSyncState>,
        rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        ledger_db: &LedgerDb,
        da_service: &Self::DaService,
        shutdown_receiver: watch::Receiver<()>,
    ) -> anyhow::Result<SequencerCreationReceipt<Self::Spec>> {
        match &rollup_config.sequencer.batch_builder {
            BatchBuilderConfig::Standard(bb_config) => {
                let (sequencer, background_handles) = SequencerBlueprint::<
                    Self,
                    M,
                    StdBatchBuilder<(Self::DaService, Self::Spec, Self::Runtime)>,
                >::new(
                    state_update_receiver.clone(),
                    da_service.clone(),
                    da_sync_state,
                    &rollup_config.storage.path,
                    ledger_db.clone(),
                    &rollup_config.sequencer.with_bb_config(bb_config.clone()),
                    shutdown_receiver,
                )
                .await?;

                Ok(SequencerCreationReceipt {
                    api_state: sequencer.api_state(),
                    axum_router: sequencer.rest_api_server(),
                    background_handles,
                    sequence_number_provider: None,
                })
            }

            BatchBuilderConfig::Preferred(bb_config) => {
                let (sequencer, background_handles) = SequencerBlueprint::<
                    Self,
                    M,
                    PreferredBatchBuilder<(Self::DaService, Self::Spec, Self::Runtime)>,
                >::new(
                    state_update_receiver.clone(),
                    da_service.clone(),
                    da_sync_state,
                    &rollup_config.storage.path,
                    ledger_db.clone(),
                    &rollup_config.sequencer.with_bb_config(bb_config.clone()),
                    shutdown_receiver,
                )
                .await?;

                Ok(SequencerCreationReceipt {
                    api_state: sequencer.api_state(),
                    axum_router: sequencer.rest_api_server(),
                    background_handles,
                    sequence_number_provider: Some(Arc::new(sequencer)),
                })
            }
        }
    }

    /// Identical to [`FullNodeBlueprint::create_new_rollup`], but with
    /// a custom [`GenesisParams`].
    #[tracing::instrument(name = "init_blueprint", skip_all)]
    async fn create_new_rollup_with_genesis_params(
        &self,
        genesis_params: GenesisParams<<Self::Runtime as RuntimeTrait<Self::Spec>>::GenesisConfig>,
        rollup_config: RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        prover_config: Option<RollupProverConfig<<Self::Spec as Spec>::InnerZkvm>>,
    ) -> anyhow::Result<Rollup<Self, M>>
    where
        <Self::Spec as Spec>::Storage: NativeStorage,
    {
        let (main_shutdown_sender, mut main_shutdown_receiver) = tokio::sync::watch::channel(());
        main_shutdown_receiver.mark_unchanged();
        let (secondary_shutdown_sender, mut secondary_shutdown_receiver) =
            tokio::sync::watch::channel(());
        secondary_shutdown_receiver.mark_unchanged();

        let operating_mode =
            <Self::Runtime as RuntimeTrait<Self::Spec>>::operating_mode(&genesis_params.runtime);
        info!(?operating_mode, "Instantiating a new rollup");

        let da_service = self
            .create_da_service(&rollup_config, secondary_shutdown_receiver.clone())
            .await;
        let da_service_handle = da_service.take_background_join_handle();
        let da_service = Arc::new(da_service);

        let mut storage_manager = self.create_storage_manager(&rollup_config)?;

        let (prover_storage, ledger_state) = storage_manager.create_bootstrap_state()?;
        let mut ledger_db = self.create_ledger_db(ledger_state)?;

        let prev_root = ledger_db
            .get_head_slot()?
            .map(|(number, _)| prover_storage.get_root_hash(number))
            .transpose()?;

        let is_genesis = prev_root.is_none();

        info!(?prev_root, is_genesis, "Recovering the state root");

        let native_stf = StfBlueprint::new();

        let (prev_state_root, genesis_state_root) = match prev_root {
            Some(prev_state_root) => {
                let (prover_storage, _ledger_state) = storage_manager.create_bootstrap_state()?;
                let genesis_state_root = prover_storage.get_root_hash(SlotNumber::GENESIS)?;

                (prev_state_root, genesis_state_root)
            }
            None => {
                info!(
                    rollup_genesis_height = rollup_config.runner.genesis_height,
                    "Rollup state is empty, performing genesis initialization. Requesting genesis DA block"
                );
                let rollup_genesis_block = da_service
                    .get_block_at(rollup_config.runner.genesis_height)
                    .await?;

                let genesis_state_root: <<Self::Spec as Spec>::Storage as Storage>::Root =
                    initialize_state::<_, _, _, Self::DaService, _>(
                        &native_stf,
                        &mut storage_manager,
                        &ledger_db,
                        rollup_genesis_block,
                        genesis_params,
                    )
                    .await?;

                (genesis_state_root.clone(), genesis_state_root)
            }
        };

        let prover_storage = if is_genesis {
            // Re-create bootstrap storage, so it fetches the latest version after initialization.
            // And can see the latest changes. Otherwise Sequencer won't be able to process any batches,
            // because genesis data won't be visible to it.
            let (prover_storage, ledger_state) = storage_manager.create_bootstrap_state()?;
            ledger_db.replace_reader(ledger_state);

            // Clearing notifications that has been produced during genesis.
            // Rollup is not running yet, so there are no subscribers.
            ledger_db.send_notifications();
            prover_storage
        } else {
            // Just using previously created bootstrap storage, because it is initialized
            prover_storage
        };

        let state_update_info = query_state_update_info(&ledger_db, prover_storage).await?;

        tracing::debug!(
            prev_root_hash = hex::encode(prev_state_root.as_ref()),
            raw_genesis_state_root = hex::encode(genesis_state_root.as_ref()),
            ?state_update_info,
            "Rollup state initialization is completed"
        );

        let (state_update_sender, state_update_receiver) =
            tokio::sync::watch::channel(state_update_info);

        let mut background_handles = vec![];
        if let Some(handle) = da_service_handle {
            background_handles.push(handle);
        }

        let visible_state_height_tracker: Box<dyn ProvableHeightTracker> = Box::new(
            MaximumProvableHeight::new(state_update_sender.subscribe(), Self::Runtime::default()),
        );

        let (sync_status_sender, sync_status_receiver) =
            tokio::sync::watch::channel(SyncStatus::START);

        let mut runner = StateTransitionRunner::new(
            rollup_config.runner.clone(),
            if prover_config.is_some() {
                Some(rollup_config.proof_manager.clone())
            } else {
                None
            },
            da_service.clone(),
            ledger_db.clone(),
            native_stf,
            storage_manager,
            state_update_sender,
            prev_state_root,
            sync_status_sender,
            visible_state_height_tracker,
            main_shutdown_receiver.clone(),
            rollup_config.monitoring.clone(),
        )
        .await?;

        let sequencer = self
            .create_sequencer(
                state_update_receiver.clone(),
                runner.da_sync_state(),
                &rollup_config,
                &ledger_db,
                &da_service,
                main_shutdown_receiver.clone(),
            )
            .await?;

        if let Some(st_info_receiver) = runner.take_st_info_receiver() {
            let prover_config = prover_config
                .expect("This code path should not be possible; this is a bug, please report it");

            let prover_service = self
                .create_prover_service(prover_config, &rollup_config, &da_service)
                .await;

            let process_manager = WorkflowProcessManager::new(
                prover_service,
                da_service.clone(),
                genesis_state_root,
                secondary_shutdown_receiver.clone(),
                st_info_receiver,
                Box::new(self.create_proof_serializer(
                    &rollup_config,
                    sequencer.sequence_number_provider.clone(),
                )?),
            );

            let workflow_task_handle = match operating_mode {
                OperatingMode::Optimistic => {
                    let prover_address = rollup_config.proof_manager.prover_address.clone();
                    let bonding_proof_service = Self::Runtime::default()
                        .proof_processor()
                        .create_bonding_proof_service(
                            prover_address,
                            state_update_receiver.clone(),
                            Self::Runtime::default(),
                        );

                    process_manager
                        .start_op_workflow_in_background(bonding_proof_service)
                        .await?
                }
                OperatingMode::Zk => {
                    process_manager
                        .start_zk_workflow_in_background(
                            rollup_config.proof_manager.aggregated_proof_block_jump,
                        )
                        .await?
                }
            };

            background_handles.push(workflow_task_handle);
        }

        let endpoints = self
            .create_endpoints(
                state_update_receiver,
                sync_status_receiver,
                &ledger_db,
                &sequencer,
                &da_service,
                &rollup_config,
            )
            .await?;

        background_handles.extend(sequencer.background_handles);

        spawn_os_signal_handler(main_shutdown_sender.clone());

        Ok(Rollup {
            runner,
            endpoints,
            shutdown_sender: main_shutdown_sender,
            secondary_shutdown_sender,
            background_handles,
        })
    }
}

/// Dependencies needed to run the rollup.
pub struct Rollup<S: FullNodeBlueprint<M>, M: ExecutionMode> {
    /// The State Transition Runner.
    #[allow(clippy::type_complexity)]
    pub runner: StateTransitionRunner<
        StfBlueprint<S::Spec, S::Runtime>,
        S::StorageManager,
        S::DaService,
        <S::Spec as Spec>::InnerZkvm,
        <S::Spec as Spec>::OuterZkvm,
    >,

    /// Server endpoints for the rollup.
    pub endpoints: RuntimeEndpoints,

    /// A way to gracefully shut down background tasks.
    pub shutdown_sender: tokio::sync::watch::Sender<()>,

    // Trigger after the runner has finished.
    secondary_shutdown_sender: tokio::sync::watch::Sender<()>,

    background_handles: Vec<tokio::task::JoinHandle<()>>,
}

impl<S: FullNodeBlueprint<M>, M: ExecutionMode> Rollup<S, M> {
    /// Runs the rollup.
    pub async fn run(self) -> anyhow::Result<()> {
        self.run_and_report_addr(None, None).await
    }

    /// Runs the rollup. Reports REST and RPC ports to the caller using the provided channel.
    pub async fn run_and_report_addr(
        self,
        rpc_addr_channel: Option<oneshot::Sender<SocketAddr>>,
        axum_addr_channel: Option<oneshot::Sender<SocketAddr>>,
    ) -> anyhow::Result<()> {
        let mut runner = self.runner;

        let rpc_addr = runner
            .start_rpc_server(self.endpoints.jsonrpsee_module)
            .await
            .context("Failed to start RPC server")?;
        if let Some(sender) = rpc_addr_channel {
            sender
                .send(rpc_addr)
                .map_err(|_| anyhow::anyhow!("Failed to send RPC address"))?;
        }

        let axum_addr = runner
            .start_axum_server(self.endpoints.axum_router)
            .await
            .context("Failed to start Axum Server")?;
        if let Some(sender) = axum_addr_channel {
            sender
                .send(axum_addr)
                .map_err(|_| anyhow::anyhow!("Failed to send Axum address"))?;
        }

        runner.run_in_process().await?;
        tracing::info!("STF Runner has completed execution");
        self.secondary_shutdown_sender.send(())?;
        for handle in self.background_handles {
            handle.await?;
        }
        for handle in self.endpoints.background_handles {
            handle.await??;
        }
        tracing::debug!("Rollup completed run");
        Ok(())
    }
}

/// A [`Sequencer`] that for a rollup built with [`RollupBlueprint`].
pub type SequencerBlueprint<B, M, Bb> = Sequencer<RollupBlueprintSequencerSpec<B, M, Bb>>;

/// The [`SequencerSpec`] of a [`SequencerBlueprint`].
#[derive(derivative::Derivative)]
#[derivative(Clone(bound = ""))]
pub struct RollupBlueprintSequencerSpec<B, M, Bb>(PhantomData<(B, M, Bb)>);

impl<B, M, Bb> SequencerSpec for RollupBlueprintSequencerSpec<B, M, Bb>
where
    B: FullNodeBlueprint<M> + Send + Sync + 'static,
    M: ExecutionMode + Send + Sync + 'static,
    Bb: BatchBuilder<Spec = B::Spec>,
{
    type BatchBuilder = Bb;
    type Da = B::DaService;
    type BatchReceipt = BatchSequencerReceipt<B::Spec>;
    type TxReceipt = TxReceiptContents<B::Spec>;
    type Event = RuntimeEventResponse<<B::Runtime as RuntimeEventProcessor>::RuntimeEvent>;
}

fn spawn_os_signal_handler(shutdown_sender: tokio::sync::watch::Sender<()>) {
    tokio::spawn(async move {
        let mut api_shutdown = shutdown_sender.subscribe();
        let mut terminate = tokio::signal::unix::signal(SignalKind::terminate())
            .expect("Failed to set up SIGTERM handler");
        let mut quit = tokio::signal::unix::signal(SignalKind::quit())
            .expect("Failed to set up SIGQUIT handler");

        tokio::select! {
            _ = tokio::signal::ctrl_c() => tracing::info!("Received Ctrl+C"),
            _ = terminate.recv() => tracing::info!("Received SIGTERM"),
            _ = quit.recv() => tracing::info!("Received SIGQUIT"),
            _ = api_shutdown.changed() => {
                tracing::debug!("Stopping OS signal handling task, as rollup has been stopped programmatically");
                return;
            }
        }
        shutdown_sender
            .send(())
            .expect("Failed to send shutdown signal");
    });
}

/// The result of [`FullNodeBlueprint::create_sequencer`].
pub struct SequencerCreationReceipt<S: Spec> {
    /// The [`ApiState`] that shall be used by REST APIs.
    ///
    /// See [`sov_modules_api::rest::HasRestApi::rest_api`].
    pub api_state: ApiState<S>,
    /// Will be passed to [`FullNodeBlueprint::create_proof_serializer`].
    ///
    /// See [`crate::proof_serializer::SovApiProofSerializer::new`].
    pub sequence_number_provider: Option<Arc<dyn SequenceNumberProvider>>,
    #[allow(missing_docs)]
    pub axum_router: axum::Router<()>,
    #[allow(missing_docs)]
    pub background_handles: Vec<JoinHandle<()>>,
}
