use std::str::FromStr;

use sov_attester_incentives::{CallMessage, Role, WrappedAttestation};
use sov_bank::{TokenId, GAS_TOKEN_ID};
use sov_mock_da::{MockValidityCond, MockValidityCondChecker};
use sov_mock_zkvm::{MockCodeCommitment, MockZkvm};
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::optimistic::Attestation;
use sov_modules_api::{CryptoSpec, PrivateKey, Spec, StateTransition, WorkingSet};
use sov_modules_stf_blueprint::TxEffect;
use sov_state::jmt::RootHash;
use sov_state::StorageRoot;
use sov_test_utils::attester_incentive_data::AttesterIncentivesMessageGenerator;
use sov_test_utils::value_setter_data::ValueSetterMessages;
use sov_test_utils::{new_test_blob_from_batch, MessageGenerator};

use crate::attester_incentives::get_first_transaction_receipt;
use crate::helpers::{
    AttesterIncentivesParams, BankParams, Da, SequencerParams, TestRollup, TestRuntime, S,
};

type TestPrivateKey = <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey;

// This tests that the `attester_incentives` module works correctly with a simple value setter module.
// The transactions of the value setter execute correcly and the state transitions are correctly stored and updated.
// This checks that the module correctly processes the attestations, rewards the attesters and updates the maximum attested height.
#[test]
fn test_honest_value_setter_process_attestation() {
    // Build a STF blueprint with the module configurations
    let mut rollup = TestRollup::new();

    let value_setter_messages = ValueSetterMessages::prepopulated();
    let value_setter = value_setter_messages.create_raw_txs::<TestRuntime<S, Da>>();

    let admin_pub_key = value_setter_messages.messages[0].admin.to_address();

    // An attester that is already bounded at genesis
    let honest_attester_pkey = TestPrivateKey::generate();
    let honest_attester_addr = honest_attester_pkey.to_address();
    let honest_attester_stake = 100;

    let seq_params = SequencerParams::default();
    let seq_da_addr = seq_params.da_address;
    let bank_params = BankParams::default();
    let token_addr = TokenId::from_str(GAS_TOKEN_ID).unwrap();

    let attester_params = AttesterIncentivesParams {
        initial_attesters: vec![(honest_attester_addr, honest_attester_stake)],
        reward_token_supply_address: [1; 32].into(),
        rollup_finality_period: 2,
        minimum_attester_bond: 100,
        minimum_challenger_bond: 100,
        maximum_attested_height: 0,
        light_client_finalized_height: 0,
        commitment_to_allowed_challenge_method: MockCodeCommitment([0; 32]),
        validity_condition_checker: MockValidityCondChecker::default(),
    };

    // Genesis
    let init_state_root = rollup.genesis(admin_pub_key, seq_params, bank_params, attester_params);

    // Execute a first transaction
    let blob = new_test_blob_from_batch(
        BatchWithId {
            txs: value_setter,
            id: [0; 32],
        },
        seq_da_addr.as_ref(),
        [2; 32],
    );

    let mut exec_vars = rollup.execution_simulation(
        2,
        init_state_root,
        vec![blob],
        0,
        Some(honest_attester_addr),
    );

    assert_eq!(exec_vars.len(), 2, "The execution simulation failed");
    let snd_res = exec_vars.pop().expect("The execution simulation failed");
    let fst_res = exec_vars.pop().expect("The execution simulation failed");

    // The first execution has succeeded
    {
        assert_eq!(snd_res.batch_receipts.len(), 1);
        assert_eq!(
            get_first_transaction_receipt(&snd_res).receipt,
            TxEffect::Successful
        );
        assert_eq!(fst_res.batch_receipts.len(), 1);
        assert_eq!(
            get_first_transaction_receipt(&fst_res).receipt,
            TxEffect::Successful
        );
    }

    // The current maximum attested height is 0 and the attester is bonded
    {
        assert_eq!(rollup.get_maximum_attested_height(), 0);

        assert_eq!(
            rollup.get_user_bond(Role::Attester, honest_attester_addr),
            honest_attester_stake
        );

        let mut working_set = WorkingSet::<S>::new(rollup.storage());
        assert_eq!(
            rollup
                .bank()
                .get_balance_of(honest_attester_addr, token_addr, &mut working_set),
            Some(0)
        );
    }

    // Attest only one transition
    // Let's try to attest the last transaction
    let (first_state_root, first_state_proof) = (
        fst_res.state_root,
        fst_res.state_proof.expect("There should be a state proof"),
    );

    let (snd_state_root, snd_state_proof) = (
        snd_res.state_root,
        snd_res.state_proof.expect("There should be a state proof"),
    );

    let attestation = Attestation {
        initial_state_root: init_state_root,
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
                honest_attester_pkey.clone(),
                CallMessage::ProcessAttestation::<S, Da>(WrappedAttestation::from(attestation)),
            )])
            .create_raw_txs::<TestRuntime<S, Da>>(),
            id: [1; 32],
        },
        seq_da_addr.as_ref(),
        [3; 32],
    );

    let exec_vars = rollup.execution_simulation(
        1,
        snd_state_root,
        vec![attestation_blob],
        2,
        Some(honest_attester_addr),
    );

    let attestation_tx = exec_vars
        .first()
        .expect("The attestation execution simulation failed");

    // Let's check that the attestation was processed correctly
    {
        assert_eq!(attestation_tx.batch_receipts.len(), 1);
        let batch_receipt = attestation_tx.batch_receipts.first().unwrap();
        assert_eq!(batch_receipt.tx_receipts.len(), 1);
        let tx_receipt = batch_receipt.tx_receipts.first().unwrap();
        assert_eq!(tx_receipt.receipt, TxEffect::Successful);

        // The current maximum attested height is 1
        assert_eq!(rollup.get_maximum_attested_height(), 1);

        // We have to check that the attester was rewarded with its stake.
        let mut working_set = WorkingSet::<S>::new(rollup.storage());
        assert_eq!(
            rollup
                .bank()
                .get_balance_of(honest_attester_addr, token_addr, &mut working_set),
            Some(honest_attester_stake)
        );
    }

    // Attest multiple transitions within one block

    // Let's try to attest the second transition and the first attestation

    let (attestation_state_root, attestation_state_proof) = (
        attestation_tx.state_root,
        attestation_tx
            .state_proof
            .clone()
            .expect("There should be a state proof"),
    );

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
                    honest_attester_pkey.clone(),
                    CallMessage::ProcessAttestation::<S, Da>(WrappedAttestation::from(
                        fst_attestation,
                    )),
                ),
                (
                    honest_attester_pkey,
                    CallMessage::ProcessAttestation::<S, Da>(WrappedAttestation::from(
                        snd_attestation,
                    )),
                ),
            ])
            .create_raw_txs::<TestRuntime<S, Da>>(),
            id: [2; 32],
        },
        seq_da_addr.as_ref(),
        [4; 32],
    );

    let exec_vars =
        rollup.execution_simulation(1, attestation_state_root, vec![attestation_blob], 3, None);

    let snd_attestation_tx = exec_vars
        .first()
        .expect("The rollup panicked while processing the second attestation");

    // Let's check that the attestation was processed correctly
    {
        assert_eq!(snd_attestation_tx.batch_receipts.len(), 1);
        let mut tx_receipts = snd_attestation_tx
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

        // We have to check that the attester was rewarded with its stake.
        let mut working_set = WorkingSet::<S>::new(rollup.storage());
        assert_eq!(
            rollup
                .bank()
                .get_balance_of(honest_attester_addr, token_addr, &mut working_set),
            Some(honest_attester_stake * 3)
        );
    }
}

// This test checks that the `attester_incentives` module works correctly with a value setter module
// for a byzantine attester.
#[test]
fn test_byzantine_value_setter_process_attestation() {
    // Build a STF blueprint with the module configurations
    let mut rollup = TestRollup::new();

    let value_setter_messages = ValueSetterMessages::prepopulated();
    let value_setter = value_setter_messages.create_raw_txs::<TestRuntime<S, Da>>();

    let admin_pub_key = value_setter_messages.messages[0].admin.to_address();

    // An attester that is already bounded at genesis
    let attester_pkey = TestPrivateKey::generate();
    let attester_addr = attester_pkey.to_address();
    let attester_stake = 100;

    // A challenger
    let challenger_pkey = TestPrivateKey::generate();
    let challenger_addr = challenger_pkey.to_address();
    let challenger_stake = 100;

    let seq_params = SequencerParams::default();
    let seq_da_addr = seq_params.da_address;
    let bank_params = BankParams {
        token_name: "TOKEN_TEST".to_string(),
        salt: 0,
        init_balance: 1000000,
        addresses_and_balances: vec![(challenger_addr, challenger_stake)],
    };
    let token_addr = TokenId::from_str(GAS_TOKEN_ID).unwrap();

    let attester_params = AttesterIncentivesParams {
        initial_attesters: vec![(attester_addr, attester_stake)],
        reward_token_supply_address: [1; 32].into(),
        rollup_finality_period: 2,
        minimum_attester_bond: 100,
        minimum_challenger_bond: 100,
        maximum_attested_height: 0,
        light_client_finalized_height: 0,
        commitment_to_allowed_challenge_method: MockCodeCommitment([0; 32]),
        validity_condition_checker: MockValidityCondChecker::default(),
    };

    // Genesis
    let init_state_root = rollup.genesis(admin_pub_key, seq_params, bank_params, attester_params);

    // Execute a first transaction
    let blob = new_test_blob_from_batch(
        BatchWithId {
            txs: value_setter,
            id: [0; 32],
        },
        seq_da_addr.as_ref(),
        [2; 32],
    );

    let exec_vars =
        rollup.execution_simulation(1, init_state_root, vec![blob], 0, Some(attester_addr));

    assert_eq!(exec_vars.len(), 1, "The execution simulation failed");
    let exec_result = exec_vars.first().expect("The execution simulation failed");

    // The first execution has succeeded
    {
        assert_eq!(exec_result.batch_receipts.len(), 1);
        assert_eq!(
            get_first_transaction_receipt(exec_result).receipt,
            TxEffect::Successful
        );
    }

    // Check that the attester is bonded
    {
        assert_eq!(
            rollup.get_user_bond(Role::Attester, attester_addr),
            attester_stake
        );

        let mut working_set = WorkingSet::<S>::new(rollup.storage());
        assert_eq!(
            rollup
                .bank()
                .get_balance_of(attester_addr, token_addr, &mut working_set),
            Some(0)
        );
    }

    // Attest only one transition

    // Let's try to attest the last transaction
    let (fst_state_root, first_state_proof) = (
        exec_result.state_root,
        exec_result
            .state_proof
            .clone()
            .expect("There should be a state proof"),
    );

    // We produce a fake attestation that has the wrong post state root
    let fake_attestation = Attestation {
        initial_state_root: init_state_root,
        slot_hash: [10; 32].into(),
        post_state_root: StorageRoot::new(RootHash([0; 32]), RootHash([0; 32])),
        proof_of_bond: sov_modules_api::optimistic::ProofOfBond {
            claimed_transition_num: 1,
            proof: first_state_proof,
        },
    };

    let fake_attestation_blob = new_test_blob_from_batch(
        BatchWithId {
            txs: AttesterIncentivesMessageGenerator::from(vec![(
                attester_pkey.clone(),
                CallMessage::ProcessAttestation::<S, Da>(WrappedAttestation::from(
                    fake_attestation,
                )),
            )])
            .create_raw_txs::<TestRuntime<S, Da>>(),
            id: [1; 32],
        },
        seq_da_addr.as_ref(),
        [2; 32],
    );

    let exec_vars =
        rollup.execution_simulation(1, fst_state_root, vec![fake_attestation_blob], 1, None);

    let attestation_res = exec_vars
        .first()
        .expect("The attestation execution simulation failed");

    let snd_state_root = attestation_res.state_root;

    // Let's check that the attester was slashed
    {
        // The transaction was successful (we need to gracefully exit to be able to update the state)
        assert_eq!(attestation_res.batch_receipts.len(), 1);

        assert_eq!(
            get_first_transaction_receipt(attestation_res).receipt,
            TxEffect::Successful
        );

        // The attester is slashed
        assert_eq!(rollup.get_bad_transition_reward(1), attester_stake);

        assert_eq!(rollup.get_user_bond(Role::Attester, attester_addr,), 0);
    }

    // A challenger can now claim the stake from the bad attestation
    // We build the challenge
    let transition = StateTransition::<Da, _> {
        initial_state_root: init_state_root,
        slot_hash: [10; 32].into(),
        final_state_root: fst_state_root,
        validity_condition: MockValidityCond { is_valid: true },
    };

    let proof = MockZkvm::create_serialized_proof(true, transition);

    // The challenger has to bond first, then he can send the attestation.
    let challenger_bond_blob = new_test_blob_from_batch(
        BatchWithId {
            txs: AttesterIncentivesMessageGenerator::from(vec![
                (
                    challenger_pkey.clone(),
                    CallMessage::BondChallenger::<S, Da>(challenger_stake),
                ),
                (challenger_pkey, CallMessage::ProcessChallenge(proof, 1)),
            ])
            .create_raw_txs::<TestRuntime<S, Da>>(),
            id: [2; 32],
        },
        seq_da_addr.as_ref(),
        [3; 32],
    );

    let exec_vars =
        rollup.execution_simulation(1, snd_state_root, vec![challenger_bond_blob], 3, None);

    let challenge_tx = exec_vars
        .first()
        .expect("The challenge execution simulation failed");

    // The challenger has successfully bonded and challenged the attester
    {
        assert_eq!(challenge_tx.batch_receipts.len(), 1);
        let mut tx_receipts = challenge_tx
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

        let mut working_set = WorkingSet::<S>::new(rollup.storage());

        // The challenger has bonded
        assert_eq!(
            rollup.get_user_bond(Role::Challenger, challenger_addr),
            challenger_stake
        );

        // The transition has been removed from the bad transition pool
        assert_eq!(rollup.get_bad_transition_reward(1), 0);

        // The challenger has been rewarded half of the pool's reward (to avoid a DoS attack)
        assert_eq!(
            rollup
                .bank()
                .get_balance_of(challenger_addr, token_addr, &mut working_set),
            Some(challenger_stake / 2)
        );
    }
}
