use sov_attester_incentives::Role;
use sov_modules_api::{Spec, StateMapAccessor, StateValueAccessor, WorkingSet};
use sov_modules_stf_blueprint::TxEffect;
use sov_rollup_interface::stf::TransactionReceipt;
use sov_state::{DefaultStorageSpec, Storage, StorageRoot};

use crate::helpers::{ExecutionSimulationVars, TestRollup, S};
mod process_attestation;

mod unbond;

fn get_first_transaction_receipt(env: &ExecutionSimulationVars) -> &TransactionReceipt<TxEffect> {
    env.batch_receipts
        .first()
        .expect("Should contain a batch receipt")
        .tx_receipts
        .first()
        .expect("Should contain a transaction receipt")
}

impl TestRollup {
    pub(crate) fn increase_and_commit_light_client_attested_height(
        &mut self,
        height: u64,
    ) -> StorageRoot<DefaultStorageSpec> {
        let mut working_set = WorkingSet::<S>::new(self.storage());
        let attester_incentives = &self.attester_incentives();

        attester_incentives
            .light_client_finalized_height
            .set(&(height), &mut working_set);

        let (mut checkpoint, _, _) = working_set.checkpoint();

        let (reads_writes, _) = checkpoint.freeze();

        let storage = self.storage();

        let new_state_root = storage
            .validate_and_commit(reads_writes, &Default::default())
            .unwrap();

        self.storage_manager().commit(storage.try_into().unwrap());

        new_state_root
    }

    pub(crate) fn get_user_bond(&mut self, role: Role, user_addr: <S as Spec>::Address) -> u64 {
        let mut working_set = WorkingSet::<S>::new(self.storage());

        match role {
            Role::Attester => self
                .attester_incentives()
                .bonded_attesters
                .get(&user_addr, &mut working_set)
                .unwrap_or_default(),
            Role::Challenger => self
                .attester_incentives()
                .bonded_challengers
                .get(&user_addr, &mut working_set)
                .unwrap_or_default(),
        }
    }

    pub(crate) fn get_bad_transition_reward(&mut self, height: u64) -> u64 {
        let mut working_set = WorkingSet::<S>::new(self.storage());

        self.attester_incentives()
            .bad_transition_pool
            .get(&height, &mut working_set)
            .unwrap_or_default()
    }

    pub(crate) fn is_attester_unbonding(&mut self, user_addr: <S as Spec>::Address) -> bool {
        let mut working_set = WorkingSet::<S>::new(self.storage());

        self.attester_incentives()
            .unbonding_attesters
            .get(&user_addr, &mut working_set)
            .is_some()
    }

    pub fn get_maximum_attested_height(&mut self) -> u64 {
        let mut working_set = WorkingSet::<S>::new(self.storage());

        self.attester_incentives()
            .maximum_attested_height
            .get(&mut working_set)
            .unwrap_or_default()
    }
}
