#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

#[cfg(feature = "native")]
mod wallet;
#[cfg(feature = "native")]
pub use endpoints::*;
use pluggable_traits::PluggableSpec;
use sov_modules_api::execution_mode::ExecutionMode;
use sov_modules_api::{BlobDataWithId, Spec};
#[cfg(feature = "native")]
mod endpoints;

pub mod pluggable_traits;
use sov_modules_stf_blueprint::Runtime;
#[cfg(feature = "native")]
pub use wallet::*;

/// Recommended default log level;
pub const DEFAULT_SOV_ROLLUP_LOGGING: &str = "debug,hyper=info,risc0_zkvm=warn,jmt=info,jsonrpsee-server=info,jsonrpsee-client=info,reqwest=info,sqlx=warn,tiny_http=warn,tower_http=info,tungstenite=info,risc0_circuit_rv32im=info,risc0_zkp::verify=info";

/// A trait defining the logical STF of the rollup.
pub trait RollupBlueprint<M: ExecutionMode>: Sized + Send + Sync {
    /// The types provided by the rollup
    type Spec: PluggableSpec + Spec;

    /// The runtime for the rollup.
    type Runtime: Runtime<Self::Spec, BlobType = BlobDataWithId> + Send + Sync + 'static;
}

#[cfg(feature = "native")]
pub use blueprint::*;

#[cfg(feature = "native")]
mod blueprint {
    use std::marker::PhantomData;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;

    use anyhow::Context;
    use async_trait::async_trait;
    use sov_db::ledger_db::LedgerDb;
    use sov_db::schema::{DeltaReader, SchemaBatch};
    use sov_modules_api::execution_mode::ExecutionMode;
    use sov_modules_api::hooks::ApplyBatchHooks;
    use sov_modules_api::rest::StorageReceiver;
    use sov_modules_api::{
        OperatingMode, ProofSerializer, RuntimeEventProcessor, RuntimeEventResponse, Spec,
        SyncStatus, Zkvm,
    };
    use sov_modules_stf_blueprint::{
        GenesisParams, Runtime as RuntimeTrait, RuntimeEndpoints, StfBlueprint, TxReceiptContents,
    };
    use sov_rollup_interface::node::da::DaService;
    use sov_rollup_interface::node::DaSyncState;
    use sov_rollup_interface::optimistic::BondingProofService;
    use sov_rollup_interface::storage::HierarchicalStorageManager;
    use sov_rollup_interface::zk::{ZkvmGuest, ZkvmHost};
    use sov_sequencer::batch_builders::BatchBuilder;
    use sov_sequencer::{Sequencer, SequencerDb, SequencerSpec};
    use sov_state::storage::NativeStorage;
    use sov_state::Storage;
    use sov_stf_runner::processes::{
        ProverService, RawGenesisStateRoot, RollupProverConfig, WorkflowProcessManager,
    };
    use sov_stf_runner::{InitVariant, RollupConfig, StateTransitionRunner};
    use tokio::sync::{oneshot, watch};

    use crate::RollupBlueprint;

    /// This trait defines how to crate all the necessary dependencies required by a rollup.
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    #[async_trait]
    pub trait FullNodeBlueprint<M: ExecutionMode>: RollupBlueprint<M>
    where
        <Self::InnerZkvmHost as ZkvmHost>::Guest:
            ZkvmGuest<Verifier = <<Self as RollupBlueprint<M>>::Spec as Spec>::InnerZkvm>,
        <Self::OuterZkvmHost as ZkvmHost>::Guest:
            ZkvmGuest<Verifier = <<Self as RollupBlueprint<M>>::Spec as Spec>::OuterZkvm>,
    {
        /// Data Availability service.
        type DaService: DaService<Spec = <Self::Spec as Spec>::Da, Error = anyhow::Error> + Clone;

        /// Host of the inner zkVM program.
        type InnerZkvmHost: ZkvmHost + Send;

        /// Host of the outer zkVM program.
        type OuterZkvmHost: ZkvmHost + Send;

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

        /// Service that is used during Attestation generation.
        type BondingProofService: BondingProofService;

        /// Gets the operating mode of the rollup (Zk or Optimistic).
        fn get_operating_mode(
            genesis: &<Self::Runtime as RuntimeTrait<Self::Spec>>::GenesisConfig,
        ) -> OperatingMode;

        /// Creates a new [`BondingProofService`] service.
        fn create_bonding_proof_service(
            &self,
            attester_address: <Self::Spec as Spec>::Address,
            storage: tokio::sync::watch::Receiver<<Self::Spec as Spec>::Storage>,
        ) -> Self::BondingProofService;

        /// Creates code commitments for the outer zkVM program.
        fn create_outer_code_commitment(
            &self,
        ) -> <<Self::ProverService as ProverService>::Verifier as Zkvm>::CodeCommitment;

        /// Creates RPC methods for the rollup.
        async fn create_endpoints(
            &self,
            storage: StorageReceiver<Self::Spec>,
            sync_status_receiver: tokio::sync::watch::Receiver<SyncStatus>,
            ledger_db: &LedgerDb,
            sequencer_db: &SequencerDb,
            da_service: &Self::DaService,
            da_sync_state: Arc<DaSyncState>,
            rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        ) -> anyhow::Result<RuntimeEndpoints>;

        /// Creates GenesisConfig from genesis files.
        #[allow(clippy::type_complexity)]
        fn create_genesis_config(
            &self,
            rt_genesis_paths: &<Self::Runtime as RuntimeTrait<Self::Spec>>::GenesisPaths,
            _rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        ) -> anyhow::Result<GenesisParams<<Self::Runtime as RuntimeTrait<Self::Spec>>::GenesisConfig>>
        {
            let rt_genesis =
                <Self::Runtime as RuntimeTrait<Self::Spec>>::genesis_config(rt_genesis_paths)?;

            Ok(GenesisParams {
                runtime: rt_genesis,
            })
        }

        /// Creates instance of [`DaService`].
        async fn create_da_service(
            &self,
            rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        ) -> Self::DaService;

        /// Creates instance of [`ProverService`].
        async fn create_prover_service(
            &self,
            prover_config: RollupProverConfig,
            rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
            da_service: &Self::DaService,
        ) -> Self::ProverService;

        /// Creates instance of [`Self::StorageManager`].
        /// Panics if initialization fails.
        fn create_storage_manager(
            &self,
            rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        ) -> anyhow::Result<Self::StorageManager>;

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
            runtime_genesis_paths: &<Self::Runtime as RuntimeTrait<Self::Spec>>::GenesisPaths,
            rollup_config: RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
            prover_config: Option<RollupProverConfig>,
        ) -> anyhow::Result<Rollup<Self, M>>
        where
            <Self::Spec as Spec>::Storage: NativeStorage,
        {
            let genesis_params =
                self.create_genesis_config(runtime_genesis_paths, &rollup_config)?;
            let operating_mode = Self::get_operating_mode(&genesis_params.runtime);
            tracing::debug!(?operating_mode, "Creating new rollup");

            let da_service = Arc::new(self.create_da_service(&rollup_config).await);
            let mut storage_manager = self.create_storage_manager(&rollup_config)?;

            let (prover_storage, ledger_state) = storage_manager.create_bootstrap_state()?;
            let mut ledger_db = self.create_ledger_db(ledger_state)?;

            let prev_root = ledger_db
                .get_head_slot()?
                .map(|(number, _)| prover_storage.get_root_hash(number.0))
                .transpose()?;

            let is_genesis = prev_root.is_none();
            let init_variant: InitVariant<_, _, _, Self::DaService> = match prev_root {
                Some(prev_state_root) => InitVariant::Initialized(prev_state_root),
                None => {
                    tracing::info!(
                        rollup_genesis_height = rollup_config.runner.genesis_height,
                        "Rollup state is empty, performing genesis initialization. Requesting genesis DA block");
                    let rollup_genesis_block = da_service
                        .get_block_at(rollup_config.runner.genesis_height)
                        .await?;
                    InitVariant::Genesis {
                        block: rollup_genesis_block,
                        genesis_params,
                    }
                }
            };
            let native_stf = StfBlueprint::new();
            let (prev_state_root, genesis_state_root): (
                <<Self::Spec as Spec>::Storage as Storage>::Root,
                RawGenesisStateRoot,
            ) = init_variant
                .initialize(&mut ledger_db, &native_stf, &mut storage_manager)
                .await?;

            let prover_storage = if is_genesis {
                // Re-create bootstrap storage, so it fetches the latest version after initialization.
                // And can see latest changes. Otherwise Sequencer won't be able to process any batches,
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
            let (api_storage_sender, api_storage_receiver) =
                tokio::sync::watch::channel(prover_storage);
            tracing::debug!(
                prev_root_hash = hex::encode(prev_state_root.as_ref()),
                raw_genesis_state_root = hex::encode(&genesis_state_root.0),
                "Rollup state initialization is completed"
            );

            let sequencer_db = SequencerDb::new(
                &rollup_config.storage.path,
                Duration::from_secs(rollup_config.sequencer.dropped_tx_ttl_secs),
            )?;

            let st_info_sender = match prover_config {
                Some(config) => {
                    let prover_service = self
                        .create_prover_service(config, &rollup_config, &da_service)
                        .await;

                    let process_manager = WorkflowProcessManager::new(
                        prover_service,
                        da_service.clone(),
                        ledger_db.clone(),
                        genesis_state_root,
                        Box::new(Self::ProofSerializer::new()),
                    );

                    let st_info_sender = match operating_mode {
                        OperatingMode::Optimistic => {
                            let prover_address = rollup_config.proof_manager.prover_address.clone();
                            let receiver = api_storage_sender.subscribe();
                            let bonding_proof_service =
                                self.create_bonding_proof_service(prover_address, receiver);

                            process_manager
                                .start_op_workflow_in_background(bonding_proof_service)
                                .await?
                        }
                        OperatingMode::Zk => {
                            let (st_info_sender, _) = process_manager
                                .start_zk_workflow_in_background(
                                    rollup_config.proof_manager.aggregated_proof_block_jump,
                                    1,
                                    1,
                                )
                                .await?;

                            st_info_sender
                        }
                    };

                    Some(st_info_sender)
                }
                None => None,
            };

            let (sync_status_sender, sync_status_receiver) = watch::channel(SyncStatus::Syncing {
                synced_da_height: 0,
                target_da_height: 0,
            });

            let runner = StateTransitionRunner::new(
                rollup_config.runner.clone(),
                da_service.clone(),
                ledger_db.clone(),
                native_stf,
                storage_manager,
                api_storage_sender,
                prev_state_root,
                st_info_sender,
                sync_status_sender,
            )
            .await?;

            let endpoints = self
                .create_endpoints(
                    api_storage_receiver,
                    sync_status_receiver,
                    &ledger_db,
                    &sequencer_db,
                    &da_service,
                    runner.da_sync_state(),
                    &rollup_config,
                )
                .await?;

            Ok(Rollup { runner, endpoints })
        }
    }

    /// Dependencies needed to run the rollup.
    pub struct Rollup<S: FullNodeBlueprint<M>, M: ExecutionMode>
    where
        <S::InnerZkvmHost as ZkvmHost>::Guest:
            ZkvmGuest<Verifier = <<S as RollupBlueprint<M>>::Spec as Spec>::InnerZkvm>,
        <S::OuterZkvmHost as ZkvmHost>::Guest:
            ZkvmGuest<Verifier = <<S as RollupBlueprint<M>>::Spec as Spec>::OuterZkvm>,
    {
        /// The State Transition Runner.
        #[allow(clippy::type_complexity)]
        pub runner: StateTransitionRunner<
            StfBlueprint<S::Spec, S::Runtime>,
            S::StorageManager,
            S::DaService,
            S::InnerZkvmHost,
            S::OuterZkvmHost,
        >,

        /// Server endpoints for the rollup.
        pub endpoints: RuntimeEndpoints,
    }

    impl<S: FullNodeBlueprint<M>, M: ExecutionMode> Rollup<S, M>
    where
        <S::InnerZkvmHost as ZkvmHost>::Guest:
            ZkvmGuest<Verifier = <<S as RollupBlueprint<M>>::Spec as Spec>::InnerZkvm>,
        <S::OuterZkvmHost as ZkvmHost>::Guest:
            ZkvmGuest<Verifier = <<S as RollupBlueprint<M>>::Spec as Spec>::OuterZkvm>,
    {
        /// Runs the rollup.
        pub async fn run(self) -> anyhow::Result<()> {
            self.run_and_report_addr(None, None).await
        }

        /// Runs the rollup. Reports RPC port to the caller using the provided channel.
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
        // Bounds required by `FullNodeBlueprint`:
        // --------------------------
        <B::InnerZkvmHost as ZkvmHost>::Guest:
            ZkvmGuest<Verifier = <<B as RollupBlueprint<M>>::Spec as Spec>::InnerZkvm>,
        <B::OuterZkvmHost as ZkvmHost>::Guest:
            ZkvmGuest<Verifier = <<B as RollupBlueprint<M>>::Spec as Spec>::OuterZkvm>,
        Bb: BatchBuilder<Spec = B::Spec>,
    {
        type BatchBuilder = Bb;
        type Da = B::DaService;
        type BatchReceipt = <B::Runtime as ApplyBatchHooks>::BatchResult;
        type TxReceipt = TxReceiptContents<B::Spec>;
        type Event = RuntimeEventResponse<<B::Runtime as RuntimeEventProcessor>::RuntimeEvent>;
    }
}
