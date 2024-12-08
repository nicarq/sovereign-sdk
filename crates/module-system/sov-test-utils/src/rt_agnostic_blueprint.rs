use std::marker::PhantomData;
use std::sync::Arc;

use sov_db::ledger_db::LedgerDb;
use sov_db::storage_manager::NativeStorageManager;
use sov_mock_da::storable::service::StorableMockDaService;
use sov_mock_da::MockDaSpec;
use sov_mock_zkvm::{MockCodeCommitment, MockZkvmHost};
use sov_modules_api::capabilities::{AuthorizationData, HasCapabilities, HasKernel};
use sov_modules_api::execution_mode::Native;
use sov_modules_api::prelude::axum::async_trait;
use sov_modules_api::rest::{HasRestApi, StateUpdateReceiver};
use sov_modules_api::{BlobDataWithId, CryptoSpec, RuntimeEndpoints, Spec, SyncStatus, ZkVerifier};
use sov_modules_rollup_blueprint::proof_serializer::SovApiProofSerializer;
use sov_modules_rollup_blueprint::{FullNodeBlueprint, RollupBlueprint, SequencerCreationReceipt};
use sov_modules_stf_blueprint::Runtime as RuntimeTrait;
use sov_rollup_interface::node::da::DaServiceWithRetries;
use sov_rollup_interface::zk::aggregated_proof::CodeCommitment;
use sov_sequencer::SequenceNumberProvider;
use sov_state::{DefaultStorageSpec, ProverStorage, Storage};
use sov_stf_runner::processes::{ParallelProverService, ProverService, RollupProverConfig};
use sov_stf_runner::RollupConfig;

type S = crate::TestSpec;

/// A basic, "vanilla" [`FullNodeBlueprint`] to be used for testing.
#[derive(Default)]
pub struct RtAgnosticBlueprint<R> {
    phantom: PhantomData<R>,
}

impl<R> RollupBlueprint<Native> for RtAgnosticBlueprint<R>
where
    R: RuntimeTrait<S>
        + HasKernel<S, BlobType = BlobDataWithId>
        + HasCapabilities<S, AuthorizationData = AuthorizationData<S>>
        + HasKernel<S, BlobType = BlobDataWithId>,
{
    type Spec = S;
    type Runtime = R;
}

#[async_trait]
impl<R> FullNodeBlueprint<Native> for RtAgnosticBlueprint<R>
where
    R: RuntimeTrait<S>
        + HasRestApi<S>
        + HasCapabilities<S, AuthorizationData = AuthorizationData<S>>
        + HasKernel<S, BlobType = BlobDataWithId>
        + 'static,
{
    type DaService = DaServiceWithRetries<StorableMockDaService>;

    type StorageManager = NativeStorageManager<
        MockDaSpec,
        ProverStorage<DefaultStorageSpec<<<Self::Spec as Spec>::CryptoSpec as CryptoSpec>::Hasher>>,
    >;

    type ProverService = ParallelProverService<
        <Self::Spec as Spec>::Address,
        <<Self::Spec as Spec>::Storage as Storage>::Root,
        <<Self::Spec as Spec>::Storage as Storage>::Witness,
        Self::DaService,
        <Self::Spec as Spec>::InnerZkvm,
        <Self::Spec as Spec>::OuterZkvm,
    >;

    type ProofSerializer = SovApiProofSerializer<Self::Spec>;

    fn create_outer_code_commitment(
        &self,
    ) -> <<Self::ProverService as ProverService>::Verifier as ZkVerifier>::CodeCommitment {
        MockCodeCommitment::default()
    }

    async fn create_endpoints(
        &self,
        state_update_receiver: StateUpdateReceiver<<Self::Spec as Spec>::Storage>,
        sync_status_receiver: tokio::sync::watch::Receiver<SyncStatus>,
        ledger_db: &LedgerDb,
        sequencer: &SequencerCreationReceipt<Self::Spec>,
        _da_service: &Self::DaService,
        rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
    ) -> anyhow::Result<RuntimeEndpoints> {
        Ok(
            sov_modules_rollup_blueprint::register_endpoints::<Self, Native>(
                state_update_receiver,
                sync_status_receiver,
                ledger_db,
                sequencer,
                rollup_config,
            )
            .await?,
        )
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
        let inner_vm = MockZkvmHost::new_non_blocking();
        let outer_vm = MockZkvmHost::new_non_blocking();

        let da_verifier = Default::default();

        ParallelProverService::new_with_default_workers(
            inner_vm,
            outer_vm,
            da_verifier,
            prover_config,
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
        _rollup_config: &RollupConfig<<Self::Spec as Spec>::Address, Self::DaService>,
        sequence_number_provider: Option<Arc<dyn SequenceNumberProvider>>,
    ) -> anyhow::Result<Self::ProofSerializer> {
        Ok(Self::ProofSerializer::new(sequence_number_provider))
    }
}
