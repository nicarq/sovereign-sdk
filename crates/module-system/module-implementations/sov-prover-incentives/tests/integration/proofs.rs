use sov_modules_api::ProofOutcome;
use sov_test_utils::{assert_matches, ProofTestCase, ProofType};

use crate::helpers::{
    build_proof, consume_gas_tx_for_signer, serialize_proof, setup, TestProverIncentives,
};

#[test]
fn test_invalid_proof() {
    let (mut runner, _, _) = setup();

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofType::Inline(serialize_proof(())),
        override_sequencer: None,
        assert: Box::new(|result, _state| {
            assert!(matches!(
                result.outcome.unwrap().outcome,
                ProofOutcome::Invalid
            ),);
        }),
    });
}

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
        input: ProofType::Inline(serialize_proof(aggregated_proof)),
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
