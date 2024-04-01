use std::rc::Rc;

use sov_attester_incentives::Role;
use sov_mock_da::MockValidityCondChecker;
use sov_mock_zkvm::crypto::private_key::Ed25519PrivateKey;
use sov_mock_zkvm::MockCodeCommitment;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::tx_verifier::RawTx;
use sov_modules_api::{CryptoSpec, DaSpec, PrivateKey, Spec, WorkingSet};
use sov_modules_stf_blueprint::TxEffect;
use sov_rollup_interface::stf::TransactionReceipt;
use sov_state::storage::StorageProof;
use sov_state::{DefaultStorageSpec, Storage, StorageRoot};
use sov_test_utils::value_setter_data::ValueSetterMessages;
use sov_test_utils::{new_test_blob_from_batch, MessageGenerator, TestPrivateKey};

use crate::helpers::{
    AttesterIncentivesParams, BankParams, Da, ExecutionSimulationVars, SequencerParams, TestRollup,
    TestRuntime, S,
};

mod byzantine_behavior;
mod process_attestation;

mod unbond;

type StorageRootAndProof = (
    StorageRoot<DefaultStorageSpec>,
    StorageProof<<<S as Spec>::Storage as Storage>::Proof>,
);

const USER_STAKE: u64 = 100;
const ROLLUP_FINALITY_PERIOD: u64 = 2;
const USER_BALANCE: u64 = 10000;

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

        let (checkpoint, _, _) = working_set.checkpoint();

        let (reads_writes, _, _) = checkpoint.freeze();

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

struct AttesterIncentivesTestHandler {
    pub(crate) admin_public_key: <S as Spec>::Address,
    pub(crate) value_setter: Vec<RawTx>,

    pub(crate) attester_private_key: <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey,
    pub(crate) challenger_private_key: <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey,

    pub(crate) attester_stake: u64,
    pub(crate) challenger_stake: u64,

    pub(crate) attester_balance: u64,
    pub(crate) challenger_balance: u64,

    pub(crate) seq_rollup_addr: <S as Spec>::Address,
    pub(crate) seq_da_addr: <Da as DaSpec>::Address,
}

impl AttesterIncentivesTestHandler {
    pub fn attester_addr(&self) -> <S as Spec>::Address {
        self.attester_private_key.to_address()
    }

    pub fn sequencer_params(&self) -> SequencerParams<S, Da> {
        SequencerParams {
            rollup_address: self.seq_rollup_addr,
            da_address: self.seq_da_addr,
            stake_amount: USER_STAKE,
            is_preferred_sequencer: true,
        }
    }

    pub fn bank_params(&self) -> BankParams {
        BankParams {
            salt: 0,
            token_name: "TOKEN_TEST".to_string(),
            init_balance: 1000000,
            addresses_and_balances: vec![
                (self.admin_public_key, USER_BALANCE),
                (
                    self.attester_private_key.to_address(),
                    self.attester_balance,
                ),
                (
                    self.challenger_private_key.to_address(),
                    self.challenger_balance,
                ),
                (self.seq_rollup_addr, USER_BALANCE),
            ],
        }
    }

    pub fn attester_incentives_params(&self) -> AttesterIncentivesParams<S, Da> {
        AttesterIncentivesParams {
            initial_attesters: vec![(self.attester_private_key.to_address(), self.attester_stake)],
            reward_token_supply_address: self.seq_rollup_addr,
            rollup_finality_period: ROLLUP_FINALITY_PERIOD,
            minimum_attester_bond: USER_STAKE,
            minimum_challenger_bond: USER_STAKE,
            maximum_attested_height: 0,
            light_client_finalized_height: 0,
            commitment_to_allowed_challenge_method: MockCodeCommitment([0; 32]),
            validity_condition_checker: MockValidityCondChecker::default(),
        }
    }

    fn byzantine_test_config() -> AttesterIncentivesTestHandler {
        // Build a STF blueprint with the module configurations
        let value_setter_messages = ValueSetterMessages::prepopulated();
        let sequencer_params = SequencerParams::default();

        AttesterIncentivesTestHandler {
            value_setter: value_setter_messages.create_raw_txs::<TestRuntime<S, Da>>(),
            admin_public_key: value_setter_messages.messages[0].admin.to_address(),
            attester_private_key: TestPrivateKey::generate(),
            challenger_private_key: TestPrivateKey::generate(),
            attester_stake: USER_STAKE,
            challenger_stake: USER_STAKE,
            attester_balance: USER_BALANCE,
            challenger_balance: USER_BALANCE,
            seq_da_addr: sequencer_params.da_address,
            seq_rollup_addr: sequencer_params.rollup_address,
        }
    }

    fn honest_attester_test_config() -> AttesterIncentivesTestHandler {
        let value_setter_messages = ValueSetterMessages::prepopulated();

        let seq_params = SequencerParams::default();

        let value_setter = value_setter_messages.create_raw_txs::<TestRuntime<S, Da>>();
        let admin_private_key: Rc<Ed25519PrivateKey> =
            value_setter_messages.messages[0].admin.clone();

        AttesterIncentivesTestHandler {
            admin_public_key: admin_private_key.to_address(),
            value_setter,
            attester_private_key: TestPrivateKey::generate(),
            challenger_private_key: TestPrivateKey::generate(),
            attester_stake: USER_STAKE,
            challenger_stake: USER_STAKE,
            attester_balance: USER_BALANCE,
            challenger_balance: USER_BALANCE,
            seq_da_addr: seq_params.da_address,
            seq_rollup_addr: seq_params.rollup_address,
        }
    }

    // Try to execute two value setter transactions in one single slot.
    fn try_execute_two_value_setter_transactions(
        &self,
        genesis_root: StorageRoot<DefaultStorageSpec>,
        rollup: &mut TestRollup,
    ) -> Vec<StorageRootAndProof> {
        let blob = new_test_blob_from_batch(
            BatchWithId {
                txs: self.value_setter.clone(),
                id: [0; 32],
            },
            self.seq_da_addr.as_ref(),
            [2; 32],
        );

        let mut exec_vars =
            rollup.execution_simulation(2, genesis_root, vec![blob], 0, Some(self.attester_addr()));

        assert_eq!(exec_vars.len(), 2, "The execution simulation failed");
        let snd_res = exec_vars.pop().expect("The execution simulation failed");
        let fst_res = exec_vars.pop().expect("The execution simulation failed");

        // Both executions have succeeded
        {
            assert_eq!(fst_res.batch_receipts.len(), 1);
            assert_eq!(
                get_first_transaction_receipt(&fst_res).receipt,
                TxEffect::Successful
            );
            assert_eq!(snd_res.batch_receipts.len(), 1);
            assert_eq!(
                get_first_transaction_receipt(&snd_res).receipt,
                TxEffect::Successful
            );
        }

        let (first_state_root, first_state_proof) = (
            fst_res.state_root,
            fst_res.state_proof.expect("There should be a state proof"),
        );

        let (snd_state_root, snd_state_proof) = (
            snd_res.state_root,
            snd_res.state_proof.expect("There should be a state proof"),
        );

        vec![
            (first_state_root, first_state_proof),
            (snd_state_root, snd_state_proof),
        ]
    }
}
