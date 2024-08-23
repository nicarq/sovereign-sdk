use sov_mock_da::{MockDaSpec, MockValidityCond};
use sov_modules_api::{
    AggregatedProofPublicData, ApiStateAccessor, InvalidProofError, ProofOutcome,
};
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{
    assert_matches, ProofAssertContext, ProofTestCase, ProofType, TestProver, TestSpec, TestUser,
};

use crate::helpers::{
    build_proof, consume_gas_tx_for_signer, serialize_proof, setup, TestProverIncentives, RT,
};

type S = TestSpec;

fn assert_slashed(
    context: ProofAssertContext<S, MockDaSpec>,
    state: &mut ApiStateAccessor<S>,
    prover: &TestUser<S>,
    slash_reason: &str,
) {
    assert_matches!(
        &context.outcome.unwrap().outcome,
        ProofOutcome::Invalid(e) if matches!(e, InvalidProofError::ProofInvalid(s) if s == slash_reason)
    );
    assert_eq!(
        TestProverIncentives::default()
            .bonded_provers
            .get(&prover.address(), state)
            .unwrap(),
        Some(0)
    );
}

fn prepare_for_slashing() -> (TestRunner<RT, S>, TestProver<S>, AggregatedProofPublicData) {
    let (mut runner, prover, other_user) = setup();

    for _ in 0..3 {
        // execute some transactions that will generate gas to reward the prover
        runner.execute(consume_gas_tx_for_signer(&other_user), None);
    }

    let aggregated_proof = runner
        .query_state(|state| build_proof(state, 1, 2, prover.user_info.address()))
        .unwrap();

    (runner, prover, aggregated_proof)
}

#[test]
fn test_invalid_proof_slashed() {
    let (mut runner, prover, _) = setup();

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofType::Inline(serialize_proof(())),
        override_sequencer: None,
        assert: Box::new(move |result, state| {
            assert_slashed(result, state, &prover.user_info, "Verification failed");
        }),
    });
}

#[test]
fn test_invalid_genesis_hash_slashed() {
    let (mut runner, prover, mut aggregated_proof) = prepare_for_slashing();
    aggregated_proof
        .genesis_state_root
        .clone_from(&aggregated_proof.final_state_root);

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofType::Inline(serialize_proof(aggregated_proof)),
        override_sequencer: None,
        assert: Box::new(move |result, state| {
            assert_slashed(
                result,
                state,
                &prover.user_info,
                "Invalid output IncorrectGenesisHash",
            );
        }),
    });
}

#[test]
fn test_invalid_initial_state_root() {
    let (mut runner, prover, mut aggregated_proof) = prepare_for_slashing();
    aggregated_proof
        .initial_state_root
        .clone_from(&aggregated_proof.final_state_root);

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofType::Inline(serialize_proof(aggregated_proof)),
        override_sequencer: None,
        assert: Box::new(move |result, state| {
            assert_slashed(
                result,
                state,
                &prover.user_info,
                "Invalid output IncorrectInitialStateRoot",
            );
        }),
    });
}

#[test]
fn test_invalid_final_slot_hash() {
    let (mut runner, prover, mut aggregated_proof) = prepare_for_slashing();
    aggregated_proof
        .final_slot_hash
        .clone_from(&aggregated_proof.initial_slot_hash);

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofType::Inline(serialize_proof(aggregated_proof)),
        override_sequencer: None,
        assert: Box::new(move |result, state| {
            assert_slashed(
                result,
                state,
                &prover.user_info,
                "Invalid output IncorrectFinalSlotHash",
            );
        }),
    });
}

#[test]
fn test_invalid_final_state_root() {
    let (mut runner, prover, mut aggregated_proof) = prepare_for_slashing();
    aggregated_proof
        .final_state_root
        .clone_from(&aggregated_proof.initial_state_root);

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofType::Inline(serialize_proof(aggregated_proof)),
        override_sequencer: None,
        assert: Box::new(move |result, state| {
            assert_slashed(
                result,
                state,
                &prover.user_info,
                "Invalid output IncorrectFinalStateRoot",
            );
        }),
    });
}

#[test]
fn test_invalid_initial_slot_hash() {
    let (mut runner, prover, mut aggregated_proof) = prepare_for_slashing();
    aggregated_proof
        .initial_slot_hash
        .clone_from(&aggregated_proof.final_slot_hash);

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofType::Inline(serialize_proof(aggregated_proof)),
        override_sequencer: None,
        assert: Box::new(move |result, state| {
            assert_slashed(
                result,
                state,
                &prover.user_info,
                "Invalid output IncorrectInitialSlotHash",
            );
        }),
    });
}

#[test]
fn test_invalid_initial_slot_number() {
    let (mut runner, prover, mut aggregated_proof) = prepare_for_slashing();
    aggregated_proof.initial_slot_number = 5555;

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofType::Inline(serialize_proof(aggregated_proof)),
        override_sequencer: None,
        assert: Box::new(move |result, state| {
            assert_slashed(
                result,
                state,
                &prover.user_info,
                "Invalid output InitialTransitionDoesNotExist",
            );
        }),
    });
}

#[test]
fn test_invalid_final_slot_number() {
    let (mut runner, prover, mut aggregated_proof) = prepare_for_slashing();
    aggregated_proof.final_slot_number = 0;

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofType::Inline(serialize_proof(aggregated_proof)),
        override_sequencer: None,
        assert: Box::new(move |result, state| {
            assert_slashed(
                result,
                state,
                &prover.user_info,
                "Invalid output FinalTransitionDoesNotExist",
            );
        }),
    });
}

#[test]
fn test_invalid_validity_condition() {
    let (mut runner, prover, mut aggregated_proof) = prepare_for_slashing();
    aggregated_proof
        .validity_conditions
        .push(borsh::to_vec(&MockValidityCond { is_valid: false }).unwrap());

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofType::Inline(serialize_proof(aggregated_proof)),
        override_sequencer: None,
        assert: Box::new(move |result, state| {
            assert_slashed(
                result,
                state,
                &prover.user_info,
                "Invalid output IncorrectValidityConditions",
            );
        }),
    });
}
