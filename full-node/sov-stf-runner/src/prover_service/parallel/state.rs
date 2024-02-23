use std::collections::HashMap;

use sov_rollup_interface::da::DaSpec;

use crate::prover_service::stf_info::BlockProof;
use crate::StateTransitionInfo;

pub(crate) enum ProverStatus<StateRoot, Witness, Da: DaSpec> {
    WitnessSubmitted(StateTransitionInfo<StateRoot, Witness, Da>),
    ProvingInProgress,
    Proved(BlockProof<Da, StateRoot>),
    Err(anyhow::Error),
}

pub(crate) struct ProverState<StateRoot, Witness, Da: DaSpec> {
    pub(crate) prover_status: HashMap<Da::SlotHash, ProverStatus<StateRoot, Witness, Da>>,
    pub(crate) pending_tasks_count: usize,
}

impl<StateRoot, Witness, Da: DaSpec> ProverState<StateRoot, Witness, Da> {
    pub(crate) fn remove(
        &mut self,
        hash: &Da::SlotHash,
    ) -> Option<ProverStatus<StateRoot, Witness, Da>> {
        self.prover_status.remove(hash)
    }

    pub(crate) fn set_to_proving(
        &mut self,
        hash: Da::SlotHash,
    ) -> Option<ProverStatus<StateRoot, Witness, Da>> {
        self.prover_status
            .insert(hash, ProverStatus::ProvingInProgress)
    }

    pub(crate) fn set_to_proved(
        &mut self,
        hash: Da::SlotHash,
        proof: Result<BlockProof<Da, StateRoot>, anyhow::Error>,
    ) -> Option<ProverStatus<StateRoot, Witness, Da>> {
        match proof {
            Ok(p) => self.prover_status.insert(hash, ProverStatus::Proved(p)),
            Err(e) => self.prover_status.insert(hash, ProverStatus::Err(e)),
        }
    }

    pub(crate) fn get_prover_status(
        &self,
        hash: &Da::SlotHash,
    ) -> Option<&ProverStatus<StateRoot, Witness, Da>> {
        self.prover_status.get(hash)
    }

    pub(crate) fn inc_task_count_if_not_busy(&mut self, num_threads: usize) -> bool {
        if self.pending_tasks_count >= num_threads {
            return false;
        }

        self.pending_tasks_count += 1;
        true
    }

    pub(crate) fn dec_task_count(&mut self) {
        assert!(self.pending_tasks_count > 0);
        self.pending_tasks_count -= 1;
    }
}
