use sov_mock_zkvm::MockZkvm;
use sov_modules_api::ProofOutcome;
use sov_test_utils::{ProofTestCase, ProofType};

use crate::helpers::{setup, TestProverIncentives};

#[test]
fn test_invalid_proof() {
    let (mut runner, _, _) = setup();

    runner.execute_proof::<TestProverIncentives>(ProofTestCase {
        input: ProofType::Inline(MockZkvm::create_serialized_proof(true, ())),
        override_sequencer: None,
        assert: Box::new(|result, _state| {
            assert!(matches!(
                result.outcome.unwrap().outcome,
                ProofOutcome::Invalid
            ),);
        }),
    });
}
