use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use sov_bank::{Bank, GAS_TOKEN_ID};
use sov_mock_da::MockDaSpec;
use sov_modules_api::{ApiStateAccessor, InvalidProofError, ProofOutcome, Spec};
use sov_prover_incentives::ProverIncentives;
use sov_test_utils::{
    assert_matches, AsUser, ProofInput, ProofTestCase, TestSpec, TransactionTestCase,
};

use crate::helpers::{
    build_proof, consume_gas_tx_for_signer, serialize_proof, setup, TestProverIncentives,
};

type S = sov_test_utils::TestSpec;

fn get_user_balance(address: &<S as Spec>::Address, state: &mut ApiStateAccessor<S>) -> u64 {
    Bank::<TestSpec>::default()
        .get_balance_of(address, GAS_TOKEN_ID, state)
        .unwrap()
        .unwrap()
}

#[derive(Clone)]
struct Reward {
    reward: Arc<AtomicU64>,
}

impl Reward {
    fn new(reward: u64) -> Self {
        Self {
            reward: Arc::new(AtomicU64::new(reward)),
        }
    }

    fn get(&self) -> u64 {
        self.reward.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn add(&self, amount: u64) {
        self.reward
            .fetch_add(amount, std::sync::atomic::Ordering::SeqCst);
    }
}

#[test]
fn test_valid_proof() {
    let (mut runner, prover, other_user) = setup();

    let prover_address = prover.user_info.address();
    let initial_balance = runner.query_state(|state| get_user_balance(&prover_address, state));

    let reward = Reward::new(0);

    for _ in 0..2 {
        let reward_clone = reward.clone();
        runner.execute_transaction(TransactionTestCase {
            input: other_user.create_plain_message::<Bank<TestSpec>>(
                sov_bank::CallMessage::CreateToken {
                    salt: 0,
                    token_name: "sov-test-token".to_string(),
                    initial_balance: 1000,
                    mint_to_address: other_user.address(),
                    authorized_minters: vec![],
                },
            ),
            assert: Box::new(move |result, _state| {
                reward_clone.add(result.gas_value_used);
            }),
        });
    }

    // We need one extra transaction so the prover see the rewards from the previous transaction.
    runner.execute(consume_gas_tx_for_signer(&other_user), None);

    let aggregated_proof = runner
        .query_state(|state| build_proof(state, 1, 2, prover.user_info.address()))
        .unwrap();

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofInput(serialize_proof(aggregated_proof)),
        override_sequencer: None,
        assert: Box::new(move |result, state| {
            assert_matches!(result.proof_receipt.outcome, ProofOutcome::Valid { .. });

            assert_eq!(
                get_user_balance(&prover_address, state),
                initial_balance - result.gas_value_used
                    + ProverIncentives::<S, MockDaSpec>::default()
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
        runner.execute(consume_gas_tx_for_signer(&other_user), None);
    }

    let aggregated_proof = runner
        .query_state(|state| build_proof(state, 1, 2, prover_address))
        .unwrap();

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofInput(serialize_proof(aggregated_proof)),
        override_sequencer: None,
        assert: Box::new(move |result, state| {
            assert_matches!(result.proof_receipt.outcome, ProofOutcome::Valid { .. });
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
            match result.proof_receipt.outcome {
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
