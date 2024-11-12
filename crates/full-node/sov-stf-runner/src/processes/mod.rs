//! Processes responsible for creating different kind of proofs.
mod op_manager;
mod prover_service;
mod zk_manager;
use std::sync::Arc;

use op_manager::attestations::AttestationsManager;
pub use prover_service::*;
use sov_db::ledger_db::LedgerDb;
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::optimistic::BondingProofService;
use sov_rollup_interface::stf::ProofSerializer;
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
    ledger_db: LedgerDb,
    genesis_state_root: RawGenesisStateRoot,
    proof_serializer: Box<dyn ProofSerializer>,
}

impl<Ps: ProverService> WorkflowProcessManager<Ps>
where
    Ps::DaService: DaService<Error = anyhow::Error>,
{
    /// Creates a new [WorkflowProcessManager].
    pub fn new(
        prover_service: Ps,
        da_service: Arc<Ps::DaService>,
        ledger_db: LedgerDb,
        genesis_state_root: RawGenesisStateRoot,
        proof_serializer: Box<dyn ProofSerializer>,
    ) -> Self {
        Self {
            prover_service,
            da_service,
            ledger_db,
            genesis_state_root,
            proof_serializer,
        }
    }

    /// Starts a process that generates aggregated proofs in the background.
    pub async fn start_zk_workflow_in_background(
        self,
        aggregated_proof_block_jump: usize,
        max_channel_size: usize,
        max_nb_of_infos_in_db: u64,
        shutdown_receiver: tokio::sync::watch::Receiver<()>,
    ) -> anyhow::Result<(
        Sender<Ps::StateRoot, Ps::Witness, <Ps::DaService as DaService>::Spec>,
        JoinHandle<()>,
    )> {
        let ledger_db = self.ledger_db.clone();
        let (st_info_sender, st_info_receiver) =
            new_stf_info_channel(self.ledger_db, max_channel_size, max_nb_of_infos_in_db).await?;

        let proof_manager = ZkProofManager::new(
            self.da_service,
            self.prover_service,
            aggregated_proof_block_jump,
            self.proof_serializer,
            self.genesis_state_root,
            st_info_receiver,
            shutdown_receiver,
        );

        let handle = proof_manager
            .post_aggregated_proof_to_da_in_background()
            .await;

        st_info_sender
            .notify_about_infos_from_db(&ledger_db)
            .await?;

        Ok((st_info_sender, handle))
    }

    /// Starts the process that generates optimistic proofs in the background.
    pub async fn start_op_workflow_in_background<Bps: BondingProofService>(
        self,
        bonding_proof_service: Bps,
        max_channel_size: usize,
        max_nb_of_infos_in_db: u64,
        shutdown_receiver: tokio::sync::watch::Receiver<()>,
    ) -> anyhow::Result<(
        Sender<Ps::StateRoot, Ps::Witness, <Ps::DaService as DaService>::Spec>,
        JoinHandle<()>,
    )> {
        let ledger_db = self.ledger_db.clone();
        let (st_info_sender, st_info_receiver) =
            new_stf_info_channel(self.ledger_db, max_channel_size, max_nb_of_infos_in_db).await?;

        let attestations_manager = AttestationsManager::new(
            st_info_receiver,
            bonding_proof_service,
            self.proof_serializer,
            self.da_service,
            shutdown_receiver,
        );
        let handle = attestations_manager
            .post_attestation_to_da_in_background()
            .await;

        st_info_sender
            .notify_about_infos_from_db(&ledger_db)
            .await?;

        Ok((st_info_sender, handle))
    }
}
