use async_trait::async_trait;
use backon::ExponentialBuilder;
use demo_stf::authentication::ModAuth;
use demo_stf::runtime::Runtime;
use sov_celestia_adapter::verifier::{CelestiaSpec, CelestiaVerifier, RollupParams};
use sov_celestia_adapter::{CelestiaConfig, CelestiaService};
use sov_db::ledger_db::LedgerDb;
use sov_db::storage_manager::NativeStorageManager;
use sov_kernels::basic::BasicKernel;
use sov_mock_zkvm::{MockCodeCommitment, MockZkVerifier, MockZkvm};
use sov_modules_api::default_spec::DefaultSpec;
use sov_modules_api::execution_mode::{ExecutionMode, Native, Zk};
use sov_modules_api::{CryptoSpec, SovApiProofSerializer, Spec};
use sov_modules_rollup_blueprint::pluggable_traits::PluggableSpec;
use sov_modules_rollup_blueprint::{FullNodeBlueprint, RollupBlueprint, WalletBlueprint};
use sov_modules_stf_blueprint::{RuntimeEndpoints, StfBlueprint};
use sov_risc0_adapter::host::Risc0Host;
use sov_risc0_adapter::Risc0Verifier;
use sov_rollup_interface::node::da::DaServiceWithRetries;
use sov_rollup_interface::zk::aggregated_proof::CodeCommitment;
use sov_rollup_interface::zk::Zkvm;
use sov_sequencer::{FairBatchBuilderConfig, SequencerDb};
use sov_state::{DefaultStorageSpec, ProverStorage, Storage, ZkStorage};
use sov_stf_runner::{ParallelProverService, ProverService, RollupConfig, RollupProverConfig};
use tokio::sync::watch;

use crate::{ROLLUP_BATCH_NAMESPACE, ROLLUP_PROOF_NAMESPACE};

/// Rollup with CelestiaDa
#[derive(Default)]
pub struct CelestiaDemoRollup<M> {
    phantom: std::marker::PhantomData<M>,
}

impl<M: ExecutionMode> RollupBlueprint<M> for CelestiaDemoRollup<M>
where
    DefaultSpec<Risc0Verifier, MockZkVerifier, M>: PluggableSpec,
{
    type Spec = DefaultSpec<Risc0Verifier, MockZkVerifier, M>;
    type DaSpec = CelestiaSpec;
    type Runtime = Runtime<Self::Spec, Self::DaSpec>;
    type Kernel = BasicKernel<Self::Spec, Self::DaSpec>;
}

#[async_trait]
impl FullNodeBlueprint<Native> for CelestiaDemoRollup<Native> {
    type DaService = DaServiceWithRetries<CelestiaService>;
    type DaConfig = CelestiaConfig;

    type InnerZkvmHost = Risc0Host<'static>;
    type OuterZkvmHost = MockZkvm;

    type StorageManager = NativeStorageManager<
        CelestiaSpec,
        ProverStorage<DefaultStorageSpec<<<Self::Spec as Spec>::CryptoSpec as CryptoSpec>::Hasher>>,
    >;

    type ProverService = ParallelProverService<
        <Self::Spec as Spec>::Address,
        <<Self::Spec as Spec>::Storage as Storage>::Root,
        <<Self::Spec as Spec>::Storage as Storage>::Witness,
        Self::DaService,
        Self::InnerZkvmHost,
        Self::OuterZkvmHost,
        StfBlueprint<
            <CelestiaDemoRollup<Zk> as RollupBlueprint<Zk>>::Spec,
            Self::DaSpec,
            <CelestiaDemoRollup<Zk> as RollupBlueprint<Zk>>::Runtime,
            <CelestiaDemoRollup<Zk> as RollupBlueprint<Zk>>::Kernel,
        >,
    >;

    type ProofSerializer = SovApiProofSerializer<Self::Spec>;

    fn create_outer_code_commitment(
        &self,
    ) -> <<Self::ProverService as ProverService>::Verifier as Zkvm>::CodeCommitment {
        MockCodeCommitment::default()
    }

    fn create_endpoints(
        &self,
        storage: watch::Receiver<<Self::Spec as Spec>::Storage>,
        ledger_db: &LedgerDb,
        sequencer_db: &SequencerDb,
        da_service: &Self::DaService,
        rollup_config: &RollupConfig<
            <Self::Spec as Spec>::Address,
            Self::DaConfig,
            FairBatchBuilderConfig<Self::DaSpec>,
        >,
    ) -> anyhow::Result<RuntimeEndpoints> {
        let mut endpoints = sov_modules_rollup_blueprint::register_endpoints::<
            Self,
            _,
            ModAuth<Self::Spec, Self::DaSpec>,
        >(
            storage.clone(),
            ledger_db,
            sequencer_db,
            da_service,
            &rollup_config.sequencer,
        )?;

        // TODO: Add issue for Sequencer level RPC injection:
        //   https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/366
        crate::eth::register_ethereum::<Self::Spec, Self::DaService>(
            da_service.clone(),
            storage.clone(),
            &mut endpoints.jsonrpsee_module,
        )?;

        Ok(endpoints)
    }

    async fn create_da_service(
        &self,
        rollup_config: &RollupConfig<
            <Self::Spec as Spec>::Address,
            Self::DaConfig,
            FairBatchBuilderConfig<Self::DaSpec>,
        >,
    ) -> Self::DaService {
        DaServiceWithRetries::with_exponential_backoff(
            CelestiaService::new(
                rollup_config.da.clone(),
                RollupParams {
                    rollup_batch_namespace: ROLLUP_BATCH_NAMESPACE,
                    rollup_proof_namespace: ROLLUP_PROOF_NAMESPACE,
                },
            )
            .await,
            // NOTE: Current exponential backoff policy defaults:
            // jitter: false, factor: 2, min_delay: 1s, max_delay: 60s, max_times: 3,
            ExponentialBuilder::default(),
        )
    }

    async fn create_prover_service(
        &self,
        prover_config: RollupProverConfig,
        rollup_config: &RollupConfig<
            <Self::Spec as Spec>::Address,
            Self::DaConfig,
            FairBatchBuilderConfig<Self::DaSpec>,
        >,
        _da_service: &Self::DaService,
    ) -> Self::ProverService {
        let inner_vm = Risc0Host::new(risc0::ROLLUP_ELF);
        let outer_vm = MockZkvm::new_non_blocking();

        let zk_stf = StfBlueprint::new();
        let zk_storage = ZkStorage::new();

        let da_verifier = CelestiaVerifier {
            rollup_batch_namespace: ROLLUP_BATCH_NAMESPACE,
            rollup_proof_namespace: ROLLUP_PROOF_NAMESPACE,
        };

        ParallelProverService::new_with_default_workers(
            inner_vm,
            outer_vm,
            zk_stf,
            da_verifier,
            prover_config,
            zk_storage,
            CodeCommitment::default(),
            rollup_config.proof_manager.prover_address,
        )
    }

    fn create_storage_manager(
        &self,
        rollup_config: &RollupConfig<
            <Self::Spec as Spec>::Address,
            Self::DaConfig,
            FairBatchBuilderConfig<Self::DaSpec>,
        >,
    ) -> anyhow::Result<Self::StorageManager> {
        NativeStorageManager::new(&rollup_config.storage.path)
    }
}

impl WalletBlueprint<Native> for CelestiaDemoRollup<Native> {}
