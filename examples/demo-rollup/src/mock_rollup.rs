use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use demo_stf::genesis_config::StorageConfig;
use demo_stf::runtime::Runtime;
use sov_db::ledger_db::LedgerDB;
use sov_db::sequencer_db::SequencerDB;
use sov_mock_da::{MockAddress, MockDaConfig, MockDaService, MockDaSpec};
use sov_modules_api::default_spec::{DefaultSpec, ZkDefaultSpec};
use sov_modules_api::Spec;
use sov_modules_rollup_blueprint::RollupBlueprint;
use sov_modules_stf_blueprint::kernels::basic::BasicKernel;
use sov_modules_stf_blueprint::StfBlueprint;
use sov_prover_storage_manager::ProverStorageManager;
use sov_risc0_adapter::host::Risc0Host;
use sov_risc0_adapter::Risc0Verifier;
use sov_rollup_interface::zk::aggregated_proof::CodeCommitment;
use sov_rollup_interface::zk::{ZkvmGuest, ZkvmHost};
use sov_state::{DefaultStorageSpec, Storage, ZkStorage};
use sov_stf_runner::{ParallelProverService, RollupConfig, RollupProverConfig};

/// Rollup with MockDa
pub struct MockDemoRollup {}

#[async_trait]
impl RollupBlueprint for MockDemoRollup {
    type DaService = MockDaService;
    type DaSpec = MockDaSpec;
    type DaConfig = MockDaConfig;
    type Vm = Risc0Host<'static>;

    type ZkSpec = ZkDefaultSpec<Risc0Verifier>;
    type NativeSpec = DefaultSpec<Risc0Verifier>;

    type StorageManager = ProverStorageManager<MockDaSpec, DefaultStorageSpec>;

    type ZkRuntime = Runtime<Self::ZkSpec, Self::DaSpec>;
    type NativeRuntime = Runtime<Self::NativeSpec, Self::DaSpec>;

    type NativeKernel = BasicKernel<Self::NativeSpec, Self::DaSpec>;
    type ZkKernel = BasicKernel<Self::ZkSpec, Self::DaSpec>;

    type ProverService = ParallelProverService<
        <<Self::NativeSpec as Spec>::Storage as Storage>::Root,
        <<Self::NativeSpec as Spec>::Storage as Storage>::Witness,
        Self::DaService,
        Self::Vm,
        StfBlueprint<
            Self::ZkSpec,
            Self::DaSpec,
            <<Self::Vm as ZkvmHost>::Guest as ZkvmGuest>::Verifier,
            Self::ZkRuntime,
            Self::ZkKernel,
        >,
    >;

    fn create_rpc_methods(
        &self,
        storage: Arc<RwLock<<Self::NativeSpec as Spec>::Storage>>,
        ledger_db: &LedgerDB,
        sequencer_db: &SequencerDB,
        da_service: &Self::DaService,
    ) -> Result<jsonrpsee::RpcModule<()>, anyhow::Error> {
        // TODO set the sequencer address
        let sequencer = MockAddress::new([0; 32]);

        #[allow(unused_mut)]
        let mut rpc_methods = sov_modules_rollup_blueprint::register_rpc::<
            Self::NativeRuntime,
            Self::NativeSpec,
            Self::DaService,
        >(
            storage.clone(),
            ledger_db,
            sequencer_db,
            da_service,
            sequencer,
        )?;

        #[cfg(feature = "experimental")]
        crate::eth::register_ethereum::<Self::NativeSpec, Self::DaService>(
            da_service.clone(),
            storage.clone(),
            &mut rpc_methods,
        )?;

        Ok(rpc_methods)
    }

    async fn create_da_service(
        &self,
        rollup_config: &RollupConfig<Self::DaConfig>,
    ) -> Self::DaService {
        MockDaService::from_config(rollup_config.da.clone())
    }

    async fn create_prover_service(
        &self,
        prover_config: RollupProverConfig,
        rollup_config: &RollupConfig<Self::DaConfig>,
        _da_service: &Self::DaService,
    ) -> Self::ProverService {
        let vm = Risc0Host::new(risc0::MOCK_DA_ELF);
        let zk_stf = StfBlueprint::new();
        let zk_storage = ZkStorage::new();
        let da_verifier = Default::default();

        ParallelProverService::new_with_default_workers(
            vm,
            zk_stf,
            da_verifier,
            prover_config,
            zk_storage,
            rollup_config.prover_service,
            CodeCommitment::default(),
        )
    }

    fn create_storage_manager(
        &self,
        rollup_config: &RollupConfig<Self::DaConfig>,
    ) -> anyhow::Result<Self::StorageManager> {
        let storage_config = StorageConfig {
            path: rollup_config.storage.path.clone(),
        };
        ProverStorageManager::new(storage_config)
    }
}
