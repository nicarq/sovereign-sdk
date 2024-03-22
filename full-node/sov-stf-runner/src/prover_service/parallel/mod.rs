mod prover;
mod state;
use std::sync::Arc;

use async_trait::async_trait;
use prover::Prover;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::services::da::DaService;
use sov_rollup_interface::stf::StateTransitionFunction;
use sov_rollup_interface::zk::aggregated_proof::CodeCommitment;
use sov_rollup_interface::zk::{ZkvmGuest, ZkvmHost};

use super::{ProverService, ProverServiceError};
use crate::config::ProverServiceConfig;
use crate::verifier::StateTransitionVerifier;
use crate::{
    ProofAggregationStatus, ProofProcessingStatus, RollupProverConfig, StateTransitionInfo,
    WitnessSubmissionStatus,
};

pub(crate) struct Verifier<Da, Vm, V>
where
    Da: DaService,
    Vm: ZkvmHost,
    V: StateTransitionFunction<<Vm::Guest as ZkvmGuest>::Verifier, Da::Spec> + Send + Sync,
{
    pub(crate) da_verifier: Da::Verifier,
    pub(crate) stf_verifier: StateTransitionVerifier<V, Da::Verifier, Vm::Guest>,
}

/// Prover service that generates proofs in parallel.
pub struct ParallelProverService<StateRoot, Witness, Da, InnerVm, OuterVm, V>
where
    StateRoot: Serialize + DeserializeOwned + Clone + AsRef<[u8]>,
    Witness: Serialize + DeserializeOwned,
    Da: DaService,
    InnerVm: ZkvmHost,
    OuterVm: ZkvmHost,
    V: StateTransitionFunction<<InnerVm::Guest as ZkvmGuest>::Verifier, Da::Spec> + Send + Sync,
{
    inner_vm: InnerVm,
    outer_vm: OuterVm,
    prover_config: Arc<RollupProverConfig>,

    zk_storage: V::PreState,
    prover_state: Prover<StateRoot, Witness, Da>,

    verifier: Arc<Verifier<Da, InnerVm, V>>,
    jump: usize,
}

impl<StateRoot, Witness, Da, InnerVm, OuterVm, V>
    ParallelProverService<StateRoot, Witness, Da, InnerVm, OuterVm, V>
where
    StateRoot: Serialize + DeserializeOwned + Clone + AsRef<[u8]> + Send + Sync + 'static,
    Witness: Serialize + DeserializeOwned + Send + Sync + 'static,
    Da: DaService,
    InnerVm: ZkvmHost,
    OuterVm: ZkvmHost,
    V: StateTransitionFunction<<InnerVm::Guest as ZkvmGuest>::Verifier, Da::Spec> + Send + Sync,
    V::PreState: Clone + Send + Sync,
{
    /// Creates a new prover.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        inner_vm: InnerVm,
        outer_vm: OuterVm,
        zk_stf: V,
        da_verifier: Da::Verifier,
        config: RollupProverConfig,
        zk_storage: V::PreState,
        num_threads: usize,
        prover_service_config: ProverServiceConfig,
        code_commitment: CodeCommitment,
    ) -> Self {
        let stf_verifier = StateTransitionVerifier::<V, Da::Verifier, InnerVm::Guest>::new(
            zk_stf,
            da_verifier.clone(),
        );

        let verifier = Arc::new(Verifier {
            da_verifier,
            stf_verifier,
        });

        Self {
            inner_vm,
            outer_vm,
            prover_config: Arc::new(config),
            prover_state: Prover::new(num_threads, code_commitment),
            jump: prover_service_config.aggregated_proof_block_jump,
            zk_storage,
            verifier,
        }
    }

    /// Creates a new prover.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_default_workers(
        inner_vm: InnerVm,
        outer_vm: OuterVm,
        zk_stf: V,
        da_verifier: Da::Verifier,
        config: RollupProverConfig,
        zk_storage: V::PreState,
        prover_service_config: ProverServiceConfig,
        code_commitment: CodeCommitment,
    ) -> Self {
        let num_cpus = num_cpus::get();
        assert!(num_cpus > 1, "Unable to create parallel prover service");

        Self::new(
            inner_vm,
            outer_vm,
            zk_stf,
            da_verifier,
            config,
            zk_storage,
            num_cpus - 1,
            prover_service_config,
            code_commitment,
        )
    }
}

#[async_trait]
impl<StateRoot, Witness, Da, InnerVm, OuterVm, V> ProverService
    for ParallelProverService<StateRoot, Witness, Da, InnerVm, OuterVm, V>
where
    StateRoot: Serialize + DeserializeOwned + Clone + AsRef<[u8]> + Send + Sync + 'static,
    Witness: Serialize + DeserializeOwned + Send + Sync + 'static,
    Da: DaService,
    InnerVm: ZkvmHost + 'static,
    OuterVm: ZkvmHost + 'static,
    V: StateTransitionFunction<<InnerVm::Guest as ZkvmGuest>::Verifier, Da::Spec>
        + Send
        + Sync
        + 'static,
    V::PreState: Clone + Send + Sync,
{
    type StateRoot = StateRoot;

    type Witness = Witness;

    type DaService = Da;

    type Verifier = <OuterVm::Guest as ZkvmGuest>::Verifier;

    fn aggregated_proof_block_jump(&self) -> usize {
        self.jump
    }

    async fn submit_state_transition_info(
        &self,
        state_transition_info: StateTransitionInfo<
            Self::StateRoot,
            Self::Witness,
            <Self::DaService as DaService>::Spec,
        >,
    ) -> WitnessSubmissionStatus {
        self.prover_state
            .submit_state_transition_info(state_transition_info)
    }

    async fn prove(
        &self,
        block_header_hash: <Da::Spec as DaSpec>::SlotHash,
    ) -> Result<ProofProcessingStatus, ProverServiceError> {
        let inner_vm = self.inner_vm.clone();
        let zk_storage = self.zk_storage.clone();

        self.prover_state.start_proving(
            block_header_hash,
            self.prover_config.clone(),
            inner_vm,
            zk_storage,
            self.verifier.clone(),
        )
    }

    async fn create_aggregated_proof(
        &self,
        block_header_hashes: &[<<Self::DaService as DaService>::Spec as DaSpec>::SlotHash],
    ) -> Result<ProofAggregationStatus, anyhow::Error> {
        self.prover_state.create_aggregated_proof(
            self.outer_vm.clone(),
            self.jump,
            block_header_hashes,
        )
    }
}
