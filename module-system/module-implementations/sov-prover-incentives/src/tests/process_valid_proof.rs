use borsh::BorshSerialize;
use sov_bank::GAS_TOKEN_ID;
use sov_mock_da::MockValidityCond;
use sov_mock_zkvm::MockZkvm;
use sov_modules_api::{
    AggregatedProofPublicData, CodeCommitment, Context, Gas, GasPrice, Spec, WorkingSet,
};

use super::helpers::get_transition_unwrap;
use crate::event::Event;
use crate::tests::helpers::{
    setup, simulate_chain_state_execution, BOND_AMOUNT, INITIAL_PROVER_BALANCE,
    MOCK_CODE_COMMITMENT, S,
};
const FIRST_SLOT_NUM: u64 = 1;
const LAST_SLOT_NUM: u64 = 2;

/// Builds a valid proof log that proves the transitions between [`FIRST_SLOT_NUM`] and [`LAST_SLOT_NUM`]
fn build_proof_log(
    module: &crate::ProverIncentives<S, sov_mock_da::MockDaSpec>,
    working_set: &mut WorkingSet<S>,
) -> AggregatedProofPublicData {
    let genesis_hash = module
        .chain_state
        .get_genesis_hash(working_set)
        .expect("Genesis hash must be set at genesis");
    let first_transition = get_transition_unwrap(FIRST_SLOT_NUM, module, working_set);
    let last_transition = get_transition_unwrap(LAST_SLOT_NUM, module, working_set);

    let vec_validity_cond = MockValidityCond { is_valid: true }.try_to_vec().unwrap();
    AggregatedProofPublicData {
        validity_conditions: vec![vec_validity_cond.clone(), vec_validity_cond],
        initial_slot_number: FIRST_SLOT_NUM,
        final_slot_number: LAST_SLOT_NUM,
        initial_state_root: genesis_hash.as_ref().to_vec(),
        genesis_state_root: genesis_hash.as_ref().to_vec(),
        final_state_root: last_transition.post_state_root().as_ref().to_vec(),
        initial_slot_hash: first_transition.slot_hash().as_ref().to_vec(),
        final_slot_hash: last_transition.slot_hash().as_ref().to_vec(),
        code_commitment: CodeCommitment(MOCK_CODE_COMMITMENT.0.to_vec()),
    }
}

/// Simulates the execution of the chain state and processes a valid proof of the transitions between
/// [`FIRST_SLOT_NUM`] and [`LAST_SLOT_NUM`] (included)
fn execute_txs_and_process_valid_proof(
    prover_address: <S as Spec>::Address,
    sequencer: <S as Spec>::Address,
    gas_used_per_step: &<S as Spec>::Gas,
    module: &crate::ProverIncentives<S, sov_mock_da::MockDaSpec>,
    working_set: WorkingSet<S>,
) -> WorkingSet<S> {
    let (mut state_checkpoint, meter, _) = working_set.checkpoint();
    // The first transition is the genesis transition
    // Then we have two more transitions
    simulate_chain_state_execution(
        module,
        sequencer,
        ((LAST_SLOT_NUM - FIRST_SLOT_NUM + 1) + 1)
            .try_into()
            .unwrap(),
        gas_used_per_step,
        &mut state_checkpoint,
    );
    let mut working_set = state_checkpoint.to_revertable(meter);

    let aggregated_proof = &build_proof_log(module, &mut working_set);

    let proof = MockZkvm::create_serialized_proof(true, aggregated_proof);
    let context = Context::<S>::new(prover_address, sequencer, LAST_SLOT_NUM + 1);

    module
        .process_proof(&proof, &context, &mut working_set)
        .expect("There should be no error processing a valid proof");

    working_set
}

// Performs a sequence of checks to ensure that the prover has been rewarded correctly
fn check_reward(
    prover_address: <S as Spec>::Address,
    gas_used_per_step: &<S as Spec>::Gas,
    module: &crate::ProverIncentives<S, sov_mock_da::MockDaSpec>,
    working_set: &mut WorkingSet<S>,
) -> u64 {
    // Compute the proof reward

    // We have proven two transitions, so the total gas used is 2 * gas_used_per_step
    let total_gas_used = gas_used_per_step.value(&GasPrice::<2>::from([1_u64; 2])) * 2;

    // Reward = total_gas_used * gas_price * (1-burn_rate)%
    let reward = module.burn_rate().apply(total_gas_used);

    // Assert that the working set contains a rewarded event
    assert_eq!(working_set.events().len(), 1);
    let event: Event<S> = working_set.take_event(0).unwrap().downcast().unwrap();

    assert_eq!(
        event,
        Event::ProcessedValidProof {
            prover: prover_address,
            reward,
        }
    );

    // Assert that the prover has been rewarded on his account
    // The outstanding balance is the initial balance plus the reward minus the bond amount
    let token_addr = GAS_TOKEN_ID;

    assert_eq!(
        module
            .bank
            .get_balance_of(prover_address, token_addr, working_set)
            .unwrap_or_default(),
        reward + INITIAL_PROVER_BALANCE - BOND_AMOUNT
    );

    // Assert that the prover's bond amount has not been burned
    assert_eq!(
        module
            .get_bond_amount(prover_address, working_set)
            .unwrap()
            .value,
        BOND_AMOUNT
    );

    reward
}

/// Checks that the prover gets penalized if he tries to prove the same transitions again
fn check_penalization_if_proven_again(
    prover_address: <S as Spec>::Address,
    sequencer: <S as Spec>::Address,
    proving_penalty: u64,
    module: &crate::ProverIncentives<S, sov_mock_da::MockDaSpec>,
    working_set: &mut WorkingSet<S>,
) {
    assert_eq!(
        module
            .last_claimed_reward
            .get(working_set)
            .expect("This slot height should be present in the claimed_rewards map"),
        LAST_SLOT_NUM,
        "The reward for the slot height {} should be claimed",
        LAST_SLOT_NUM
    );

    let proof_log = build_proof_log(module, working_set);
    let proof = MockZkvm::create_serialized_proof(true, proof_log);

    let context = Context::<S>::new(prover_address, sequencer, LAST_SLOT_NUM + 2);
    module
        .process_proof(&proof, &context, working_set)
        .expect("The proof should not be rejected");

    // Assert that the working set contains a penalized event
    assert_eq!(working_set.events().len(), 1);
    let event: Event<S> = working_set.take_event(0).unwrap().downcast().unwrap();
    assert_eq!(
        event,
        Event::ProverPenalized {
            prover: prover_address,
            amount: proving_penalty,
            reason: crate::event::PenalizationReason::ProofAlreadyProcessed
        }
    );

    // Assert that the prover's bond amount has been penalized
    assert_eq!(
        module
            .get_bond_amount(prover_address, working_set)
            .unwrap()
            .value,
        BOND_AMOUNT - proving_penalty
    );
}

fn check_unbonding(
    prover_address: <S as Spec>::Address,
    sequencer: <S as Spec>::Address,
    expected_amount_withdrawn: u64,
    old_balance: u64,
    module: &crate::ProverIncentives<S, sov_mock_da::MockDaSpec>,
    working_set: &mut WorkingSet<S>,
) {
    let context = Context::<S>::new(prover_address, sequencer, LAST_SLOT_NUM + 2);
    module
        .unbond_prover(&context, working_set)
        .expect("The proof should not be rejected");

    assert_eq!(working_set.events().len(), 1);
    let event: Event<S> = working_set.take_event(0).unwrap().downcast().unwrap();
    assert_eq!(
        event,
        Event::UnBondedProver {
            prover: prover_address,
            amount_withdrawn: expected_amount_withdrawn
        }
    );

    // Check that the prover has been unbonded
    assert_eq!(
        module
            .get_bond_amount(prover_address, working_set)
            .unwrap()
            .value,
        0
    );

    // Check the amount on the prover's balance
    assert_eq!(
        module
            .bank
            .get_balance_of(prover_address, GAS_TOKEN_ID, working_set)
            .unwrap(),
        old_balance + expected_amount_withdrawn
    );
}

#[test]
/// Macro-test for the happy path of processing a valid proof.
fn test_valid_proof() {
    let (module, prover_address, sequencer, working_set) = setup();

    let gas_used_per_step = <S as Spec>::Gas::from([1_u64; 2]);

    // Process a valid proof
    let mut working_set = execute_txs_and_process_valid_proof(
        prover_address,
        sequencer,
        &gas_used_per_step,
        &module,
        working_set,
    );

    let reward = check_reward(
        prover_address,
        &gas_used_per_step,
        &module,
        &mut working_set,
    );

    // Now we have to check we can unbond
    check_unbonding(
        prover_address,
        sequencer,
        BOND_AMOUNT,
        INITIAL_PROVER_BALANCE - BOND_AMOUNT + reward,
        &module,
        &mut working_set,
    );
}

#[test]
/// Macro-test for the happy path of processing a valid proof with penalization.
fn test_valid_proof_with_penalization() {
    let (module, prover_address, sequencer, working_set) = setup();

    let gas_used_per_step = <S as Spec>::Gas::from([1_u64; 2]);

    // Process a valid proof
    let mut working_set = execute_txs_and_process_valid_proof(
        prover_address,
        sequencer,
        &gas_used_per_step,
        &module,
        working_set,
    );

    let reward = check_reward(
        prover_address,
        &gas_used_per_step,
        &module,
        &mut working_set,
    );

    let proving_penalty = module
        .proving_penalty
        .get(&mut working_set)
        .expect("The proving penalty should be set at genesis");

    // Now we have to check that we cannot prove the same transitions again
    check_penalization_if_proven_again(
        prover_address,
        sequencer,
        proving_penalty,
        &module,
        &mut working_set,
    );

    // Now we have to check we can unbond
    check_unbonding(
        prover_address,
        sequencer,
        BOND_AMOUNT - proving_penalty,
        INITIAL_PROVER_BALANCE - BOND_AMOUNT + reward,
        &module,
        &mut working_set,
    );
}
