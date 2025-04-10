//! Processes responsible for creating different kind of proofs.
mod op_manager;
mod prover_service;
mod zk_manager;
use std::num::NonZero;
use std::sync::Arc;

use op_manager::attestations::AttestationsManager;
pub use prover_service::*;
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::optimistic::BondingProofService;
use sov_rollup_interface::stf::ProofSerializer;
use tokio::sync::watch;
use tokio::task::JoinHandle;
pub use zk_manager::*;
mod stf_info_manager;
pub use stf_info_manager::*;

/// The [`crate::StateTransitionRunner`] executes batches of transactions and produces [`StateTransitionInfo`] data.
/// This data is then consumed by an external process. For a zk-rollup, the process generates aggregated proofs,
/// while for an optimistic rollup, it produces attestations.
/// [`WorkflowProcessManager`] is responsible for creating and managing this process.
pub struct WorkflowProcessManager<Ps: ProverService> {
    prover_service: Ps,
    da_service: Arc<Ps::DaService>,
    genesis_state_root: Ps::StateRoot,
    shutdown_receiver: watch::Receiver<()>,
    proof_serializer: Box<dyn ProofSerializer>,
    stf_info_receiver: Receiver<Ps::StateRoot, Ps::Witness, <Ps::DaService as DaService>::Spec>,
}

impl<Ps> WorkflowProcessManager<Ps>
where
    Ps: ProverService,
    Ps::DaService: DaService<Error = anyhow::Error>,
{
    /// Creates a new [`WorkflowProcessManager`].
    pub fn new(
        prover_service: Ps,
        da_service: Arc<Ps::DaService>,
        genesis_state_root: Ps::StateRoot,
        shutdown_receiver: watch::Receiver<()>,
        stf_info_receiver: Receiver<Ps::StateRoot, Ps::Witness, <Ps::DaService as DaService>::Spec>,
        proof_serializer: Box<dyn ProofSerializer>,
    ) -> Self {
        Self {
            prover_service,
            da_service,
            genesis_state_root,
            shutdown_receiver,
            stf_info_receiver,
            proof_serializer,
        }
    }

    /// Starts a process that generates aggregated proofs in the background.
    pub async fn start_zk_workflow_in_background(
        self,
        aggregated_proof_block_jump: NonZero<usize>,
    ) -> anyhow::Result<JoinHandle<()>> {
        let proof_manager = ZkProofManager::new(
            self.da_service,
            self.prover_service,
            aggregated_proof_block_jump,
            self.proof_serializer,
            self.genesis_state_root,
            self.stf_info_receiver,
            self.shutdown_receiver,
        );

        Ok(proof_manager
            .post_aggregated_proof_to_da_in_background()
            .await)
    }

    /// Starts the process that generates optimistic proofs in the background.
    pub async fn start_op_workflow_in_background<Bps: BondingProofService>(
        self,
        bonding_proof_service: Bps,
    ) -> anyhow::Result<JoinHandle<()>> {
        let attestations_manager = AttestationsManager::new(
            self.stf_info_receiver,
            bonding_proof_service,
            self.proof_serializer,
            self.da_service,
            self.shutdown_receiver,
        );
        Ok(attestations_manager
            .post_attestation_to_da_in_background()
            .await)
    }
}
