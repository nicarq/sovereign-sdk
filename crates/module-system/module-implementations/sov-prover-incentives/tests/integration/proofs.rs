use sov_modules_api::{Gas, InvalidProofError, ProofOutcome, Spec};
use sov_prover_incentives::ProverIncentives;
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{
    assert_matches, AtomicNumber, ProofInput, ProofTestCase, TransactionTestCase,
};

use crate::helpers::{
    build_proof, consume_gas_tx_for_signer, serialize_proof, setup, TestProverIncentives, RT,
};

type S = sov_test_utils::TestSpec;

#[test]
fn test_valid_proof() {
    let (mut runner, prover, other_user) = setup();

    let prover_address = prover.user_info.address();
    let initial_balance = runner
        .query_visible_state(|state| TestRunner::<RT, S>::bank_gas_balance(&prover_address, state))
        .unwrap();

    let reward = AtomicNumber::new(0);

    for _ in 0..2 {
        let reward_clone = reward.clone();
        runner.execute_transaction(TransactionTestCase {
            input: consume_gas_tx_for_signer(&other_user),
            assert: Box::new(move |result, _state| {
                reward_clone.add(result.gas_value_used);
            }),
        });
    }

    // We need one extra transaction so the prover see the rewards from the previous transaction.
    runner.execute(consume_gas_tx_for_signer(&other_user));

    let aggregated_proof = runner
        .query_visible_state(|state| build_proof(state, 1, 2, prover.user_info.address()))
        .unwrap();

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofInput(serialize_proof(aggregated_proof)),
        assert: Box::new(move |result, state| {
            assert_matches!(
                result.proof_receipt.unwrap().outcome,
                ProofOutcome::Valid { .. }
            );

            assert_eq!(
                TestRunner::<RT, S>::bank_gas_balance(&prover_address, state).unwrap(),
                initial_balance - result.gas_value_used
                    + ProverIncentives::<S>::default()
                        .burn_rate()
                        .apply(reward.get())
            );
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
        runner.execute(consume_gas_tx_for_signer(&other_user));
    }

    let aggregated_proof = runner
        .query_visible_state(|state| build_proof(state, 1, 2, prover_address))
        .unwrap();

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofInput(serialize_proof(aggregated_proof)),
        assert: Box::new(move |result, state| {
            assert_matches!(
                result.proof_receipt.unwrap().outcome,
                ProofOutcome::Valid { .. }
            );
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
                    .unwrap()
                    .map(|v| v.get()),
                Some(2)
            );
        }),
    });

    let aggregated_proof = runner
        .query_visible_state(|state| build_proof(state, 1, 2, prover_address))
        .unwrap();

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofInput(serialize_proof(aggregated_proof)),
        assert: Box::new(move |result, state| {
            match result.proof_receipt.clone().unwrap().outcome {
                ProofOutcome::Invalid(InvalidProofError::ProverPenalized(_)) => {}
                _ => panic!("Expected prover to be penalized"),
            }

            let prover_incentives = TestProverIncentives::default();
            let penalty = prover_incentives
                .proving_penalty
                .get(state)
                .unwrap()
                .unwrap();
            let gas_price = <<S as Spec>::Gas as Gas>::Price::try_from(
                result.proof_receipt.unwrap().gas_price.clone(),
            )
            .unwrap();

            let bonded_amount = prover_incentives
                .bonded_provers
                .get(&prover_address, state)
                .unwrap()
                .unwrap();
            assert_eq!(bonded_amount, prover.bond - penalty.value(&gas_price));
        }),
    });
}
