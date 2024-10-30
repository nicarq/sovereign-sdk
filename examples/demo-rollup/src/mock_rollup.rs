use std::sync::Arc;

use async_trait::async_trait;
use demo_stf::runtime::{EthereumToRollupAddressConverter, Runtime};
use sov_db::ledger_db::LedgerDb;
use sov_db::storage_manager::NativeStorageManager;
use sov_mock_da::storable::service::StorableMockDaService;
use sov_mock_da::MockDaSpec;
use sov_mock_zkvm::{MockCodeCommitment, MockZkVerifier, MockZkvm};
use sov_modules_api::default_spec::DefaultSpec;
use sov_modules_api::execution_mode::{ExecutionMode, Native, Zk};
use sov_modules_api::higher_kinded_types::Generic;
use sov_modules_api::{CryptoSpec, OperatingMode, Spec, SyncStatus, Zkvm};
use sov_modules_rollup_blueprint::pluggable_traits::PluggableSpec;
use sov_modules_rollup_blueprint::proof_serializer::SovApiProofSerializer;
use sov_modules_rollup_blueprint::{FullNodeBlueprint, RollupBlueprint};
use sov_modules_stf_blueprint::{Runtime as RuntimeTrait, RuntimeEndpoints, StfBlueprint};
use sov_risc0_adapter::host::Risc0Host;
use sov_risc0_adapter::Risc0Verifier;
use sov_rollup_interface::node::da::DaServiceWithRetries;
use sov_rollup_interface::node::DaSyncState;
use sov_rollup_interface::zk::aggregated_proof::CodeCommitment;
use sov_sequencer::SequencerDb;
use sov_state::{DefaultStorageSpec, ProverStorage, Storage, ZkStorage};
use sov_stf_runner::processes::{ParallelProverService, ProverService, RollupProverConfig};
use sov_stf_runner::RollupConfig;
use tokio::sync::watch;

/// Rollup with MockDa
#[derive(Default)]
pub struct MockDemoRollup<M> {
    phantom: std::marker::PhantomData<M>,
}

impl<M: ExecutionMode> RollupBlueprint<M> for MockDemoRollup<M>
where
    DefaultSpec<MockDaSpec, Risc0Verifier, MockZkVerifier, M>: PluggableSpec,
    EthereumToRollupAddressConverter:
        TryInto<<DefaultSpec<MockDaSpec, Risc0Verifier, MockZkVerifier, M> as Spec>::Address>,
{
    type Spec = DefaultSpec<MockDaSpec, Risc0Verifier, MockZkVerifier, M>;
    type Runtime = Runtime<Self::Spec>;
}

#[async_trait]
impl FullNodeBlueprint<Native> for MockDemoRollup<Native> {
    type DaService = DaServiceWithRetries<StorableMockDaService>;
    type InnerZkvmHost = Risc0Host<'static>;
    type OuterZkvmHost = MockZkvm;

    type StorageManager = NativeStorageManager<
        MockDaSpec,
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
            <Self::Spec as Generic>::With<Zk>,
            <MockDemoRollup<Zk> as RollupBlueprint<Zk>>::Runtime,
        >,
    >;

    type ProofSerializer = SovApiProofSerializer<Self::Spec>;

    fn get_operating_mode(
        genesis: &<Self::Runtime as RuntimeTrait<Self::Spec>>::GenesisConfig,
    ) -> OperatingMode {
        genesis.chain_state.operating_mode
    }

    fn create_outer_code_commitment(
        &self,
    ) -> <<Self::ProverService as ProverService>::Verifier as Zkvm>::CodeCommitment {
        MockCodeCommitment::default()
    }

    async fn create_endpoints(
        &self,
        storage: watch::Receiver<<Self::Spec as Spec>::Storage>,
        sync_status_receiver: watch::Receiver<SyncStatus>,
        ledger_db: &LedgerDb,
        sequencer_db: &SequencerDb,
        da_service: &Self::DaService,
        da_sync_state: Arc<DaSyncState>,
        rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
    ) -> anyhow::Result<RuntimeEndpoints> {
        let mut endpoints = sov_modules_rollup_blueprint::register_endpoints::<Self, Native>(
            storage.clone(),
            sync_status_receiver,
            ledger_db,
            sequencer_db,
            da_service,
            da_sync_state,
            &rollup_config.sequencer,
            &rollup_config.runner,
        )
        .await?;

        // TODO: Add issue for Sequencer level RPC injection:
        //   https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/366
        crate::eth::register_ethereum::<Self::Spec, Self::DaService, Self::Runtime>(
            da_service.clone(),
            storage,
            &mut endpoints.jsonrpsee_module,
        )?;

        Ok(endpoints)
    }

    async fn create_da_service(
        &self,
        rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
    ) -> Self::DaService {
        DaServiceWithRetries::new_fast(
            StorableMockDaService::from_config(rollup_config.da.clone()).await,
        )
    }

    async fn create_prover_service(
        &self,
        prover_config: RollupProverConfig,
        rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        _da_service: &Self::DaService,
    ) -> Self::ProverService {
        let inner_vm = if let RollupProverConfig::Skip = prover_config {
            Risc0Host::new(b"")
        } else {
            let elf = std::fs::read(risc0::MOCK_DA_PATH)
                .unwrap_or_else(|e| {
                    panic!(
                        "Could not read guest elf file from `{}`. {}",
                        risc0::MOCK_DA_PATH,
                        e
                    )
                })
                .leak();
            Risc0Host::new(elf)
        };

        let outer_vm = MockZkvm::new_non_blocking();
        let zk_stf = StfBlueprint::new();
        let zk_storage = ZkStorage::new();
        let da_verifier = Default::default();

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
        rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
    ) -> anyhow::Result<Self::StorageManager> {
        NativeStorageManager::new(&rollup_config.storage.path)
    }

    fn create_proof_serializer(
        &self,
        rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        sequencer_db: &SequencerDb,
    ) -> anyhow::Result<Self::ProofSerializer> {
        Ok(Self::ProofSerializer::new(
            sequencer_db,
            rollup_config.sequencer.is_preferred_sequencer(),
        ))
    }
}
