#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

mod runtime_rpc;
mod wallet;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
pub use runtime_rpc::*;
use sov_db::ledger_db::LedgerDB;
use sov_db::schema::{CacheDb, ChangeSet};
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::runtime::capabilities::{Kernel, KernelSlotHooks};
use sov_modules_api::{DaSpec, Spec, Zkvm};
use sov_modules_stf_blueprint::{GenesisParams, Runtime as RuntimeTrait, StfBlueprint};
use sov_rollup_interface::services::da::DaService;
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_rollup_interface::zk::{ZkvmGuest, ZkvmHost};
use sov_sequencer::SequencerDb;
use sov_state::storage::NativeStorage;
use sov_state::Storage;
use sov_stf_runner::{
    InitVariant, ProofManager, ProverService, RollupConfig, RollupProverConfig,
    StateTransitionRunner,
};
use tokio::sync::{oneshot, watch};
pub use wallet::*;

/// This trait defines how to crate all the necessary dependencies required by a rollup.
#[async_trait]
pub trait RollupBlueprint: Sized + Send + Sync {
    /// Data Availability service.
    type DaService: DaService<Spec = Self::DaSpec, Error = anyhow::Error> + Clone + Send + Sync;
    /// A specification for the types used by a DA layer.
    type DaSpec: DaSpec;
    /// Data Availability config.
    type DaConfig: Send + Sync;

    /// Host of the inner zkVM program.
    type InnerZkvmHost: ZkvmHost + Send;

    /// Host of the outer zkVM program.
    type OuterZkvmHost: ZkvmHost + Send;

    /// Context for Zero Knowledge environment.
    type ZkSpec: Spec;
    /// Context for Native environment.
    type NativeSpec: Spec;

    /// Manager for the native storage lifecycle.
    type StorageManager: HierarchicalStorageManager<
        Self::DaSpec,
        StfState = <Self::NativeSpec as Spec>::Storage,
        StfChangeSet = <<<Self as RollupBlueprint>::NativeSpec as Spec>::Storage as Storage>::ChangeSet,
        LedgerState = CacheDb,
        LedgerChangeSet = ChangeSet,
    >;

    /// Runtime for the Zero Knowledge environment.
    type ZkRuntime: RuntimeTrait<Self::ZkSpec, Self::DaSpec> + Default;
    /// Runtime for the Native environment.
    type NativeRuntime: RuntimeTrait<Self::NativeSpec, Self::DaSpec> + Default + Send + Sync;

    /// The kernel for the native environment.
    type NativeKernel: KernelSlotHooks<Self::NativeSpec, Self::DaSpec, Batch = BatchWithId>
        + Default
        + Send
        + Sync;
    /// The kernel for the Zero Knowledge environment.
    type ZkKernel: KernelSlotHooks<Self::ZkSpec, Self::DaSpec, Batch = BatchWithId> + Default;

    /// Prover service.
    type ProverService: ProverService<
        StateRoot = <<Self::NativeSpec as Spec>::Storage as Storage>::Root,
        Witness = <<Self::NativeSpec as Spec>::Storage as Storage>::Witness,
        DaService = Self::DaService,
    >;

    /// Creates code commitments for the outer zkVM program.
    fn create_outer_code_commitment(
        &self,
    ) -> <<Self::ProverService as ProverService>::Verifier as Zkvm>::CodeCommitment;

    /// Creates RPC methods for the rollup.
    fn create_rpc_methods(
        &self,
        storage: watch::Receiver<<Self::NativeSpec as Spec>::Storage>,
        ledger_db: &LedgerDB,
        sequencer_db: &SequencerDb,
        da_service: &Self::DaService,
        rollup_config: &RollupConfig<Self::DaConfig>,
    ) -> Result<jsonrpsee::RpcModule<()>, anyhow::Error>;

    /// Creates GenesisConfig from genesis files.
    #[allow(clippy::type_complexity)]
    fn create_genesis_config(
        &self,
        rt_genesis_paths: &<Self::NativeRuntime as RuntimeTrait<
            Self::NativeSpec,
            Self::DaSpec,
        >>::GenesisPaths,
        kernel_genesis: <Self::NativeKernel as Kernel<Self::NativeSpec, Self::DaSpec>>::GenesisConfig,
        _rollup_config: &RollupConfig<Self::DaConfig>,
    ) -> anyhow::Result<
        GenesisParams<
            <Self::NativeRuntime as RuntimeTrait<Self::NativeSpec, Self::DaSpec>>::GenesisConfig,
            <Self::NativeKernel as Kernel<Self::NativeSpec, Self::DaSpec>>::GenesisConfig,
        >,
    > {
        let rt_genesis =
            <Self::NativeRuntime as RuntimeTrait<Self::NativeSpec, Self::DaSpec>>::genesis_config(
                rt_genesis_paths,
            )?;

        Ok(GenesisParams {
            runtime: rt_genesis,
            kernel: kernel_genesis,
        })
    }

    /// Creates instance of [`DaService`].
    async fn create_da_service(
        &self,
        rollup_config: &RollupConfig<Self::DaConfig>,
    ) -> Self::DaService;

    /// Creates instance of [`ProverService`].
    async fn create_prover_service(
        &self,
        prover_config: RollupProverConfig,
        rollup_config: &RollupConfig<Self::DaConfig>,
        da_service: &Self::DaService,
    ) -> Self::ProverService;

    /// Creates instance of [`Self::StorageManager`].
    /// Panics if initialization fails.
    fn create_storage_manager(
        &self,
        rollup_config: &RollupConfig<Self::DaConfig>,
    ) -> Result<Self::StorageManager, anyhow::Error>;

    /// Creates instance of a LedgerDB.
    fn create_ledger_db(
        &self,
        ledger_state: <Self::StorageManager as HierarchicalStorageManager<Self::DaSpec>>::LedgerState,
    ) -> anyhow::Result<LedgerDB> {
        LedgerDB::with_cache_db(ledger_state)
    }

    /// Creates a new rollup.
    async fn create_new_rollup(
        &self,
        runtime_genesis_paths: &<Self::NativeRuntime as RuntimeTrait<
            Self::NativeSpec,
            Self::DaSpec,
        >>::GenesisPaths,
        kernel_genesis_config: <Self::NativeKernel as Kernel<Self::NativeSpec, Self::DaSpec>>::GenesisConfig,
        rollup_config: RollupConfig<Self::DaConfig>,
        prover_config: Option<RollupProverConfig>,
    ) -> Result<Rollup<Self>, anyhow::Error>
    where
        <Self::NativeSpec as Spec>::Storage: NativeStorage,
    {
        let da_service = Arc::new(self.create_da_service(&rollup_config).await);
        let relative_da_genesis_block = da_service
            .get_block_at(rollup_config.runner.genesis_height)
            .await?;

        let prover_service = match prover_config {
            Some(c) => Some(
                self.create_prover_service(c, &rollup_config, &da_service)
                    .await,
            ),
            None => None,
        };

        let genesis_config = self.create_genesis_config(
            runtime_genesis_paths,
            kernel_genesis_config,
            &rollup_config,
        )?;

        let mut storage_manager = self.create_storage_manager(&rollup_config)?;
        let (prover_storage, ledger_state) = storage_manager.create_bootstrap_state()?;
        let ledger_db = self.create_ledger_db(ledger_state)?;

        let sequencer_db = SequencerDb::new(&rollup_config.storage.path)?;

        let prev_root = ledger_db
            .get_head_slot()?
            .map(|(number, _)| prover_storage.get_root_hash(number.0))
            .transpose()?;

        let init_variant = match prev_root {
            Some(root_hash) => InitVariant::Initialized(root_hash),
            None => InitVariant::Genesis {
                block: relative_da_genesis_block,
                genesis_params: genesis_config,
            },
        };

        let rpc_storage = tokio::sync::watch::channel(prover_storage);
        // We pass "bootstrap" storage here,
        // as it will be replaced with the latest on after first processed block.
        let rpc_methods = self.create_rpc_methods(
            rpc_storage.1,
            &ledger_db,
            &sequencer_db,
            &da_service,
            &rollup_config,
        )?;

        let native_stf = StfBlueprint::new();

        let proof_manager = ProofManager::new(
            da_service.clone(),
            prover_service,
            ledger_db.clone(),
            self.create_outer_code_commitment(),
        );

        let runner = StateTransitionRunner::new(
            rollup_config.runner,
            da_service,
            ledger_db,
            native_stf,
            storage_manager,
            rpc_storage.0,
            init_variant,
            proof_manager,
        )?;

        Ok(Rollup {
            runner,
            rpc_methods,
        })
    }
}

/// Dependencies needed to run the rollup.
pub struct Rollup<S: RollupBlueprint> {
    /// The State Transition Runner.
    #[allow(clippy::type_complexity)]
    pub runner: StateTransitionRunner<
        StfBlueprint<
            S::NativeSpec,
            S::DaSpec,
            <<S::InnerZkvmHost as ZkvmHost>::Guest as ZkvmGuest>::Verifier,
            S::NativeRuntime,
            S::NativeKernel,
        >,
        S::StorageManager,
        S::DaService,
        S::InnerZkvmHost,
        S::ProverService,
    >,
    /// RPC methods for the rollup.
    pub rpc_methods: jsonrpsee::RpcModule<()>,
}

impl<S: RollupBlueprint> Rollup<S> {
    /// Runs the rollup.
    pub async fn run(self) -> Result<(), anyhow::Error> {
        self.run_and_report_rpc_port(None).await
    }

    /// Runs the rollup. Reports rpc port to the caller using the provided channel.
    pub async fn run_and_report_rpc_port(
        self,
        channel: Option<oneshot::Sender<SocketAddr>>,
    ) -> anyhow::Result<()> {
        let mut runner = self.runner;
        runner.start_rpc_server(self.rpc_methods, channel).await;
        runner.run_in_process().await?;
        Ok(())
    }
}
