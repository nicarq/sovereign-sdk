use sov_modules_api::{InvalidProofError, ProofOutcome};
use sov_test_utils::{assert_matches, ProofInput, ProofTestCase};

use crate::helpers::{
    build_proof, consume_gas_tx_for_signer, serialize_proof, setup, TestProverIncentives,
};

#[test]
fn test_valid_proof() {
    let (mut runner, prover, other_user) = setup();

    for _ in 0..3 {
        // execute some transactions that will generate gas to reward the prover
        runner.execute(consume_gas_tx_for_signer(&other_user), None);
    }

    let aggregated_proof = runner
        .query_state(|state| build_proof(state, 1, 2, prover.user_info.address()))
        .unwrap();

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofInput(serialize_proof(aggregated_proof)),
        override_sequencer: None,
        assert: Box::new(move |result, state| {
            assert_matches!(result.outcome.unwrap().outcome, ProofOutcome::Valid { .. });
            // Can't do this yet because it's current hard to get information about rewards/gas usage/etc for proofs
            // This info will be added to the outcome/receipt and then we can add this assertion
            // assert!(
            //     Bank::<S>::default()
            //         .get_balance_of(&prover.user_info.address(), GAS_TOKEN_ID, state)
            //         .unwrap()
            //         .unwrap()
            //         > prover.user_info.available_balance
            // );
            assert_eq!(
                TestProverIncentives::default()
                    .bonded_provers
                    .get(&prover.user_info.address(), state)
                    .unwrap(),
                Some(prover.bond),
                "Bonded amount should not have changed"
            );
        }),
    });
}

#[test]
fn test_valid_proof_penalized_if_reward_already_claimed() {
    let (mut runner, prover, other_user) = setup();
    let prover_address = prover.user_info.address();

    for _ in 0..3 {
        // execute some transactions that will consume gas to reward the prover
        runner.execute(consume_gas_tx_for_signer(&other_user), None);
    }

    let aggregated_proof = runner
        .query_state(|state| build_proof(state, 1, 2, prover_address))
        .unwrap();

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofInput(serialize_proof(aggregated_proof)),
        override_sequencer: None,
        assert: Box::new(move |result, state| {
            assert_matches!(result.outcome.unwrap().outcome, ProofOutcome::Valid { .. });
            assert_eq!(
                TestProverIncentives::default()
                    .bonded_provers
                    .get(&prover_address, state)
                    .unwrap(),
                Some(prover.bond),
                "Bonded amount should not have changed"
            );
            assert_eq!(
                TestProverIncentives::default()
                    .last_claimed_reward
                    .get(state)
                    .unwrap(),
                Some(2)
            );
        }),
    });

    let aggregated_proof = runner
        .query_state(|state| build_proof(state, 1, 2, prover_address))
        .unwrap();

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofInput(serialize_proof(aggregated_proof)),
        override_sequencer: None,
        assert: Box::new(move |result, state| {
            match result.outcome.unwrap().outcome {
                ProofOutcome::Invalid(InvalidProofError::ProverPenalized(_)) => {}
                _ => panic!("Expected prover to be penalized"),
            }

            let prover_incentives = TestProverIncentives::default();
            let penalty = prover_incentives
                .proving_penalty
                .get(state)
                .unwrap()
                .unwrap();
            let bonded_amount = prover_incentives
                .bonded_provers
                .get(&prover_address, state)
                .unwrap()
                .unwrap();
            assert_eq!(bonded_amount, prover.bond - penalty);
        }),
    });
}
