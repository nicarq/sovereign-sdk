use sov_bank::GAS_TOKEN_ID;
use sov_mock_da::{MockDaSpec, MockValidityCond};
use sov_mock_zkvm::MockZkvm;
use sov_modules_api::{Context, Spec, TxGasMeter, WorkingSet};
use sov_prover_storage_manager::SimpleStorageManager;
use sov_rollup_interface::zk::StateTransitionPublicData;

use crate::call::AttesterIncentiveErrors;
use crate::tests::helpers::{
    commit_get_new_storage, setup, ExecutionSimulationVars, BOND_AMOUNT, INITIAL_USER_BALANCE,
    INIT_HEIGHT,
};
use crate::SlashingReason;

type S = sov_test_utils::TestSpec;

/// Test that given an invalid transition, a challenger can successfully challenge it and get rewarded
#[test]
fn test_valid_challenge() {
    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = SimpleStorageManager::new(tmpdir.path());
    let storage = storage_manager.create_storage();
    let working_set = WorkingSet::new(storage.clone());
    let (module, attester_address, challenger_address, sequencer, working_set) = setup(working_set);

    // Simulate the execution of a chain, with the genesis hash and two transitions after.
    // Update the chain_state module and the optimistic module accordingly
    let state_checkpoint = working_set.checkpoint().0;
    commit_get_new_storage(storage, state_checkpoint, &mut storage_manager);
    let (mut exec_vars, state_checkpoint) = ExecutionSimulationVars::execute(
        3,
        &module,
        &mut storage_manager,
        &sequencer,
        &attester_address,
    );

    let mut working_set = state_checkpoint.to_revertable(TxGasMeter::unmetered());

    let _ = exec_vars.pop().unwrap();
    let transition_1 = exec_vars.pop().unwrap();
    let initial_transition = exec_vars.pop().unwrap();

    module
        .bond_user_helper(
            BOND_AMOUNT,
            &challenger_address,
            crate::call::Role::Challenger,
            &mut working_set,
        )
        .unwrap();

    // Assert that the challenger has the correct bond amount before processing the proof
    assert_eq!(
        module
            .get_bond_amount(
                challenger_address,
                crate::call::Role::Challenger,
                &mut working_set
            )
            .value,
        BOND_AMOUNT
    );

    // Set a bad transition to get a reward from
    module
        .bad_transition_pool
        .set(&(INIT_HEIGHT + 1), &BOND_AMOUNT, &mut working_set);

    let context = Context::<S>::new(challenger_address, sequencer, INIT_HEIGHT + 2);

    {
        let transition = StateTransitionPublicData::<MockDaSpec, _> {
            initial_state_root: initial_transition.state_root,
            slot_hash: [1; 32].into(),
            final_state_root: transition_1.state_root,
            validity_condition: MockValidityCond { is_valid: true },
        };

        let proof = &MockZkvm::create_serialized_proof(true, transition);

        module
            .process_challenge(
                &context,
                proof.as_slice(),
                &(INIT_HEIGHT + 1),
                &mut working_set,
            )
            .expect("Should not fail");

        // Check that the challenger was rewarded
        assert_eq!(
            module
                .bank
                .get_balance_of(&challenger_address, GAS_TOKEN_ID, &mut working_set)
                .unwrap(),
            INITIAL_USER_BALANCE - BOND_AMOUNT + module.burn_rate().apply(BOND_AMOUNT),
            "The challenger should have been rewarded"
        );

        // Check that the challenge set is empty
        assert_eq!(
            module
                .bad_transition_pool
                .get(&(INIT_HEIGHT + 1), &mut working_set),
            None,
            "The transition should have disappeared"
        );
    }

    {
        // Now try to unbond the challenger
        module
            .unbond_challenger(&context, &mut working_set)
            .expect("The challenger should be able to unbond");

        // Check the final balance of the challenger
        assert_eq!(
            module
                .bank
                .get_balance_of(&challenger_address, GAS_TOKEN_ID, &mut working_set)
                .unwrap(),
            INITIAL_USER_BALANCE + module.burn_rate().apply(BOND_AMOUNT),
            "The challenger should have been unbonded"
        );
    }
}

fn invalid_proof_helper(
    context: &Context<S>,
    proof: &Vec<u8>,
    reason: SlashingReason,
    challenger_address: <S as Spec>::Address,
    module: &crate::AttesterIncentives<S, MockDaSpec>,
    working_set: &mut WorkingSet<S>,
) {
    // Let's bond the challenger and try to publish a false challenge
    module
        .bond_user_helper(
            BOND_AMOUNT,
            &challenger_address,
            crate::call::Role::Challenger,
            working_set,
        )
        .expect("Should be able to bond");

    module
        .process_challenge(context, proof.as_slice(), &(INIT_HEIGHT + 1), working_set)
        .expect("Since the challenger is slashed this should exit gracefully");

    // We get the last event from the working set to check the slashing reason
    let mut events = working_set.take_events();
    let slash_event = events.pop().unwrap();
    let slash_event = slash_event.downcast::<crate::Event<S>>().unwrap();

    // Check the error raised
    assert_eq!(
        slash_event,
        crate::Event::UserSlashed {
            address: challenger_address,
            reason
        },
        "The challenge processing should fail with an invalid proof error"
    );
}

#[test]
fn test_invalid_challenge() {
    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = SimpleStorageManager::new(tmpdir.path());
    let storage = storage_manager.create_storage();
    let working_set = WorkingSet::new(storage.clone());
    let (module, attester_address, challenger_address, sequencer, working_set) = setup(working_set);

    // Simulate the execution of a chain, with the genesis hash and two transitions after.
    // Update the chain_state module and the optimistic module accordingly
    let state_checkpoint = working_set.checkpoint().0;
    commit_get_new_storage(storage, state_checkpoint, &mut storage_manager);
    let (mut exec_vars, state_checkpoint) = ExecutionSimulationVars::execute(
        3,
        &module,
        &mut storage_manager,
        &sequencer,
        &attester_address,
    );
    let mut working_set = state_checkpoint.to_revertable(TxGasMeter::unmetered());

    let _ = exec_vars.pop().unwrap();
    let transition_1 = exec_vars.pop().unwrap();
    let initial_transition = exec_vars.pop().unwrap();

    // Set a bad transition to get a reward from
    module
        .bad_transition_pool
        .set(&(INIT_HEIGHT + 1), &BOND_AMOUNT, &mut working_set);

    let context = Context::<S>::new(challenger_address, sequencer, INIT_HEIGHT + 2);
    let transition: StateTransitionPublicData<MockDaSpec, _> = StateTransitionPublicData {
        initial_state_root: initial_transition.state_root,
        slot_hash: [1; 32].into(),
        final_state_root: transition_1.state_root,
        validity_condition: MockValidityCond { is_valid: true },
    };

    {
        // A valid proof
        let proof = MockZkvm::create_serialized_proof(true, &transition);

        let err = module
            .process_challenge(
                &context,
                proof.as_slice(),
                &(INIT_HEIGHT + 1),
                &mut working_set,
            )
            .unwrap_err();

        // Check the error raised
        assert_eq!(
            err,
            AttesterIncentiveErrors::UserNotBonded,
            "The challenge processing should fail with an unbonded error"
        );
    }

    // Invalid proofs
    {
        // An invalid proof
        let proof = &MockZkvm::create_serialized_proof(false, &transition);

        invalid_proof_helper(
            &context,
            proof,
            SlashingReason::InvalidProofOutputs,
            challenger_address,
            &module,
            &mut working_set,
        );

        // Bad slot hash
        let bad_transition = StateTransitionPublicData::<MockDaSpec, _> {
            initial_state_root: initial_transition.state_root,
            slot_hash: [2; 32].into(),
            final_state_root: transition_1.state_root,
            validity_condition: MockValidityCond { is_valid: true },
        };

        // An invalid proof
        let proof = &MockZkvm::create_serialized_proof(true, bad_transition);

        invalid_proof_helper(
            &context,
            proof,
            SlashingReason::TransitionInvalid,
            challenger_address,
            &module,
            &mut working_set,
        );

        // Bad validity condition
        let bad_transition = StateTransitionPublicData::<MockDaSpec, _> {
            initial_state_root: initial_transition.state_root,
            slot_hash: [1; 32].into(),
            final_state_root: transition_1.state_root,
            validity_condition: MockValidityCond { is_valid: false },
        };

        let proof = &MockZkvm::create_serialized_proof(true, bad_transition);

        invalid_proof_helper(
            &context,
            proof,
            SlashingReason::TransitionInvalid,
            challenger_address,
            &module,
            &mut working_set,
        );

        // Bad initial root
        let bad_transition = StateTransitionPublicData::<MockDaSpec, _> {
            initial_state_root: transition_1.state_root,
            slot_hash: [1; 32].into(),
            final_state_root: transition_1.state_root,
            validity_condition: MockValidityCond { is_valid: true },
        };

        let proof = &MockZkvm::create_serialized_proof(true, bad_transition);

        invalid_proof_helper(
            &context,
            proof,
            SlashingReason::InvalidInitialHash,
            challenger_address,
            &module,
            &mut working_set,
        );
    }
}
