//! Processes responsible for creating different kind of proofs.
mod prover_service;
mod zk_manager;
use std::sync::Arc;

pub use prover_service::*;
use sov_db::ledger_db::LedgerDb;
use sov_rollup_interface::node::da::DaService;
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

    /// Starts process that generates aggregated proofs in the background.
    pub async fn start_zk_workflow_in_background(
        self,
        aggregated_proof_block_jump: usize,
        max_channel_size: usize,
        max_nb_of_infos_in_db: u64,
    ) -> anyhow::Result<(
        Sender<Ps::StateRoot, Ps::Witness, <Ps::DaService as DaService>::Spec>,
        JoinHandle<()>,
    )> {
        let (st_info_sender, st_info_receiver) =
            new_stf_info_channel(self.ledger_db, max_channel_size, max_nb_of_infos_in_db).await?;

        let proof_manager = ZkProofManager::new(
            self.da_service,
            self.prover_service,
            aggregated_proof_block_jump,
            self.proof_serializer,
            self.genesis_state_root,
            st_info_receiver,
        );

        let handle = proof_manager
            .post_aggregated_proof_to_da_in_background()
            .await;

        Ok((st_info_sender, handle))
    }

    /// Starts process that generates optimistic proofs in the background.
    pub async fn start_op_workflow_in_background(
        self,
    ) -> anyhow::Result<Sender<Ps::StateRoot, Ps::Witness, <Ps::DaService as DaService>::Spec>>
    {
        let (st_info_sender, mut st_info_receiver) =
            new_stf_info_channel(self.ledger_db, 1, 2).await?;

        tokio::spawn(async move {
            loop {
                _ = st_info_receiver.read_next().await;
            }
        });

        Ok(st_info_sender)
    }
}
