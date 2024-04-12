use std::vec;

use sov_attester_incentives::{CallMessage, Role, WrappedAttestation};
use sov_bank::GAS_TOKEN_ID;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::optimistic::Attestation;
use sov_modules_api::WorkingSet;
use sov_modules_stf_blueprint::TxEffect;
use sov_state::{DefaultStorageSpec, StorageRoot};
use sov_test_utils::attester_incentive_data::AttesterIncentivesMessageGenerator;
use sov_test_utils::runtime::TestRuntime;
use sov_test_utils::{new_test_blob_from_batch, MessageGenerator};

use super::{AttesterIncentivesTestHandler, StorageRootAndProof, USER_BALANCE};
use crate::helpers::{Da, ExecutionSimulationVars, TestRollup, S};

impl AttesterIncentivesTestHandler {
    // The current maximum attested height is 0 and the attester is bonded
    fn check_initial_attestation_conditions(&self, rollup: &mut TestRollup) {
        assert_eq!(rollup.get_maximum_attested_height(), 0);

        assert_eq!(
            rollup.get_user_bond(Role::Attester, self.attester_addr()),
            self.attester_stake
        );

        let mut working_set = WorkingSet::<S>::new(rollup.storage());
        assert_eq!(
            rollup
                .bank()
                .get_balance_of(&self.attester_addr(), GAS_TOKEN_ID, &mut working_set),
            Some(self.attester_balance - self.attester_stake)
        );
    }

    // Checks that the first attestation was processed correctly
    fn check_first_attestation_processing(
        &self,
        attestation_tx: &ExecutionSimulationVars,
        honest_attester_new_balance: u64,
        rollup: &mut TestRollup,
    ) {
        assert_eq!(attestation_tx.batch_receipts.len(), 1);
        let batch_receipt = attestation_tx.batch_receipts.first().unwrap();
        assert_eq!(batch_receipt.tx_receipts.len(), 1);
        let tx_receipt = batch_receipt.tx_receipts.first().unwrap();
        assert_eq!(tx_receipt.receipt, TxEffect::Successful);

        // We have to check that the attester was rewarded with its stake.
        let mut working_set = WorkingSet::<S>::new(rollup.storage());
        assert_eq!(
            rollup
                .bank()
                .get_balance_of(&self.attester_addr(), GAS_TOKEN_ID, &mut working_set),
            Some(honest_attester_new_balance)
        );
    }

    // Attest only the first transition and check that the attestation was processed correctly
    fn try_attest_first_transition(
        &self,
        genesis_root: StorageRoot<DefaultStorageSpec>,
        state_roots_and_proofs: Vec<StorageRootAndProof>,
        honest_attester_balance: u64,
        rollup: &mut TestRollup,
    ) -> (u64, StorageRootAndProof) {
        assert!(state_roots_and_proofs.len() >= 2);
        let (first_state_root, first_state_proof) = state_roots_and_proofs[0].clone();
        let (snd_state_root, _snd_state_proof) = state_roots_and_proofs[1].clone();

        let attestation = Attestation {
            initial_state_root: genesis_root,
            slot_hash: [10; 32].into(),
            post_state_root: first_state_root,
            proof_of_bond: sov_modules_api::optimistic::ProofOfBond {
                claimed_transition_num: 1,
                proof: first_state_proof,
            },
        };

        let attestation_blob = new_test_blob_from_batch(
            BatchWithId {
                txs: AttesterIncentivesMessageGenerator::from(vec![(
                    self.attester_private_key.clone(),
                    CallMessage::ProcessAttestation::<S, Da>(WrappedAttestation::from(attestation)),
                )])
                .create_raw_txs::<TestRuntime<S, Da>>(),
                id: [1; 32],
            },
            self.seq_da_addr.as_ref(),
            [3; 32],
        );

        let exec_vars = rollup.execution_simulation(
            1,
            snd_state_root,
            vec![attestation_blob],
            2,
            Some(self.attester_addr()),
        );

        let attestation_tx = exec_vars
            .first()
            .expect("The attestation execution simulation failed");

        // The new attester balance is the initial attester balance minus the gas cost of the transaction
        // plus the burn rate applied to the amount of gas proved in the attestation
        let gas_proved = (self.num_value_setter_txs() as u64) * rollup.gas_per_transaction();
        let burn_rate = rollup.burn_rate();
        // The new attester balance is the initial attester balance minus the gas cost of the transaction
        // plus the burn rate applied to the amount of gas proved in the attestation
        let new_attester_balance =
            honest_attester_balance - rollup.gas_per_transaction() + burn_rate.apply(gas_proved);

        self.check_first_attestation_processing(attestation_tx, new_attester_balance, rollup);

        (
            new_attester_balance,
            (
                attestation_tx.state_root,
                attestation_tx
                    .state_proof
                    .clone()
                    .expect("Should have a state proof"),
            ),
        )
    }

    // Checks that the second and third attestations were processed correctly
    fn check_second_and_third_attestation_processing(
        &self,
        attestation_exec_res: &ExecutionSimulationVars,
        honest_attester_balance: u64,
        rollup: &mut TestRollup,
    ) {
        assert_eq!(attestation_exec_res.batch_receipts.len(), 1);
        let mut tx_receipts = attestation_exec_res
            .batch_receipts
            .first()
            .unwrap()
            .tx_receipts
            .clone();
        assert_eq!(tx_receipts.len(), 2);
        let snd_tx_receipt = tx_receipts.pop().unwrap();
        let fst_tx_receipt = tx_receipts.pop().unwrap();
        assert_eq!(fst_tx_receipt.receipt, TxEffect::Successful);
        assert_eq!(snd_tx_receipt.receipt, TxEffect::Successful);

        // The current maximum attested height is 3
        assert_eq!(rollup.get_maximum_attested_height(), 3);

        // We have to check that the attester was rewarded correctly.
        let mut working_set = WorkingSet::<S>::new(rollup.storage());

        let gas_proved_first_attestation =
            self.num_value_setter_txs() as u64 * rollup.gas_per_transaction();
        let gas_proved_second_attestation = rollup.gas_per_transaction();
        let burn_rate = rollup.burn_rate();
        assert_eq!(
            rollup
                .bank()
                .get_balance_of(&self.attester_addr(), GAS_TOKEN_ID, &mut working_set),
            // Formula: new_balance = old_balance + burn_rate * (gas_proved_first_attestation + gas_proved_second_attestation) - tx_cost
            Some(
                honest_attester_balance
                    + burn_rate.apply(gas_proved_first_attestation)
                    + burn_rate.apply(gas_proved_second_attestation)
                    - 2 * rollup.gas_per_transaction()
            )
        );
    }

    // Attest multiple transitions within one block
    // Let's try to attest the second transition and the first attestation
    fn try_attest_second_transition_and_first_attestation(
        &self,
        state_roots_and_proofs: Vec<StorageRootAndProof>,
        honest_attester_balance: u64,
        rollup: &mut TestRollup,
    ) {
        assert!(state_roots_and_proofs.len() >= 3);
        let (first_state_root, _first_state_proof) = state_roots_and_proofs[0].clone();
        let (snd_state_root, snd_state_proof) = state_roots_and_proofs[1].clone();
        let (attestation_state_root, attestation_state_proof) = state_roots_and_proofs[2].clone();

        let fst_attestation = Attestation {
            initial_state_root: first_state_root,
            slot_hash: [20; 32].into(),
            post_state_root: snd_state_root,
            proof_of_bond: sov_modules_api::optimistic::ProofOfBond {
                claimed_transition_num: 2,
                proof: snd_state_proof,
            },
        };

        let snd_attestation = Attestation {
            initial_state_root: snd_state_root,
            slot_hash: [30; 32].into(),
            post_state_root: attestation_state_root,
            proof_of_bond: sov_modules_api::optimistic::ProofOfBond {
                claimed_transition_num: 3,
                proof: attestation_state_proof,
            },
        };

        let attestation_blob = new_test_blob_from_batch(
            BatchWithId {
                txs: AttesterIncentivesMessageGenerator::from(vec![
                    (
                        self.attester_private_key.clone(),
                        CallMessage::ProcessAttestation::<S, Da>(WrappedAttestation::from(
                            fst_attestation,
                        )),
                    ),
                    (
                        self.attester_private_key.clone(),
                        CallMessage::ProcessAttestation::<S, Da>(WrappedAttestation::from(
                            snd_attestation,
                        )),
                    ),
                ])
                .create_raw_txs::<TestRuntime<S, Da>>(),
                id: [2; 32],
            },
            self.seq_da_addr.as_ref(),
            [4; 32],
        );

        let exec_vars =
            rollup.execution_simulation(1, attestation_state_root, vec![attestation_blob], 3, None);

        let attestation_transition = exec_vars
            .first()
            .expect("The rollup panicked while processing the second attestation");

        self.check_second_and_third_attestation_processing(
            attestation_transition,
            honest_attester_balance,
            rollup,
        );
    }
}

// This tests that the `attester_incentives` module works correctly with a simple value setter module.
// The transactions of the value setter execute correcly and the state transitions are correctly stored and updated.
// This checks that the module correctly processes the attestations, rewards the attesters and updates the maximum attested height.
#[test]
fn test_honest_value_setter_process_attestation() {
    // Build a STF blueprint with the module configurations
    let mut rollup = TestRollup::new();

    let test_handler = AttesterIncentivesTestHandler::honest_attester_test_config();

    // Genesis
    let init_state_root = rollup.genesis(
        test_handler.admin_public_key,
        test_handler.sequencer_params(),
        test_handler.bank_params(),
        test_handler.attester_incentives_params(),
    );

    test_handler.check_initial_attestation_conditions(&mut rollup);

    let mut state_roots_and_proofs =
        test_handler.try_execute_two_value_setter_transactions(init_state_root, &mut rollup);

    let (new_attester_balance, (post_attestation_state_root, post_attestation_state_proof)) =
        test_handler.try_attest_first_transition(
            init_state_root,
            state_roots_and_proofs.clone(),
            USER_BALANCE - test_handler.attester_stake,
            &mut rollup,
        );

    state_roots_and_proofs.push((post_attestation_state_root, post_attestation_state_proof));

    test_handler.try_attest_second_transition_and_first_attestation(
        state_roots_and_proofs,
        new_attester_balance,
        &mut rollup,
    );
}
