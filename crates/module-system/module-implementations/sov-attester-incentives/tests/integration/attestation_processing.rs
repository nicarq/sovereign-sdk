use sov_attester_incentives::ProcessAttestationErrors;
use sov_mock_da::MockDaSpec;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{Error, ProofOutcome, Spec, StateAccessorError, TxEffect};
use sov_test_utils::generators::attester_incentive::TestAttestationMessageError;
use sov_test_utils::runtime::sov_attester_incentives::{AttesterIncentives, CallMessage, Event};
use sov_test_utils::{
    assert_matches, AsUser, ProofTestCase, ProofType, TestAttester, TransactionTestCase,
    TEST_DEFAULT_USER_STAKE,
};

use super::helpers::{setup, TestRuntimeEvent, S};
use crate::helpers::{
    build_proof, consume_gas_tx_for_signer, make_attestation_blob, TestAttesterIncentives,
};

#[test]
fn test_process_valid_attestation() {
    let (mut runner, genesis_attester, _, other_user) = setup();

    for _ in 0..5 {
        runner.execute(consume_gas_tx_for_signer(&other_user), None);
    }

    let attester_addr = genesis_attester.user_info.address();

    let attestation_proof_1 = runner
        .query_state(|state| build_proof(state, 1, &attester_addr))
        .unwrap();

    let attest_slot_1 = create_test_case(
        genesis_attester.clone(),
        make_attestation_blob(attestation_proof_1),
    );

    let attestation_proof_2 = runner
        .query_state(|state| build_proof(state, 2, &attester_addr))
        .unwrap();

    let attest_slot_2 = create_test_case(
        genesis_attester.clone(),
        make_attestation_blob(attestation_proof_2),
    );

    let attestation_proof_3 = runner
        .query_state(|state| build_proof(state, 3, &attester_addr))
        .unwrap();

    let attest_slot_3 =
        create_test_case(genesis_attester, make_attestation_blob(attestation_proof_3));

    runner
        .advance_slots(5)
        .execute_proof::<TestAttesterIncentives>(attest_slot_1)
        .execute_proof::<TestAttesterIncentives>(attest_slot_2)
        .execute_proof::<TestAttesterIncentives>(attest_slot_3);
}

fn create_test_case(
    genesis_attester: TestAttester<S>,
    serialized_attestation: Vec<u8>,
) -> ProofTestCase<S, MockDaSpec> {
    ProofTestCase {
        input: ProofType::Inline(serialized_attestation),
        override_sequencer: None,
        assert: Box::new(move |result, state| {
            assert_matches!(result.outcome.unwrap().outcome, ProofOutcome::Valid { .. });

            assert_eq!(
                TestAttesterIncentives::default()
                    .bonded_attesters
                    .get(&genesis_attester.user_info.address(), state)
                    .unwrap(),
                Some(genesis_attester.bond),
                "Bonded amount should not have changed"
            );

            // TODO #1292: check rewards.
        }),
    }
}

// TODO: #1262
#[ignore]
#[test]
fn test_burn_on_invalid_attestation() {
    let (mut runner, mut genesis_attester, _, _) = setup();

    let genesis_attester_address = genesis_attester.user_info.address();
    let genesis_attester_bond = genesis_attester.bond;

    let invalid_bond_proof_no_slash = TransactionTestCase {
        input: genesis_attester
            .test_process_attestation(Err(TestAttestationMessageError::InvalidProofOfBond)),
        assert: Box::new(move |result, state| {
            assert!(matches!(
                &result.outcome,
                TxEffect::Reverted(e) if *e == Error::ModuleError(
                    ProcessAttestationErrors::<StateAccessorError<<S as Spec>::Gas>>::InvalidBondingProof.into(),
                )
            ));
            // Assert that the attester was not slashed
            assert_eq!(
                AttesterIncentives::<S, MockDaSpec>::default()
                    .get_attester_bond_amount(genesis_attester_address, state)
                    .unwrap_infallible()
                    .value,
                genesis_attester_bond,
            );
        }),
    };
    let valid_attestation = TransactionTestCase {
        input: genesis_attester.test_process_attestation(Ok(())),
        assert: Box::new(|result, _state| {
            // Check that the attestation succeeded
            assert!(result.events.iter().any(|event| matches!(
                event,
                TestRuntimeEvent::attester_incentives(Event::ProcessedValidAttestation { .. })
            )));
        }),
    };
    let invalid_initial_state_slashed = TransactionTestCase {
        input: genesis_attester
            .test_process_attestation(Err(TestAttestationMessageError::InvalidInitialStateRoot)),
        assert: Box::new(move |result, state| {
            // Check that the attestation resulted in slashing
            assert!(result.events.iter().any(|event| matches!(
                event,
                TestRuntimeEvent::attester_incentives(Event::UserSlashed { .. })
            )));
            // Assert that the attester was slashed
            assert_eq!(
                AttesterIncentives::<S, MockDaSpec>::default()
                    .get_attester_bond_amount(genesis_attester_address, state)
                    .unwrap_infallible()
                    .value,
                0,
            );
            // Check that the invalid attestation is not part of the challengeable set.
            // (Since it has the wrong pre-state, no one will be fooled by it so we don't reward challengers)
            assert!(
                AttesterIncentives::<S, MockDaSpec>::default()
                    .bad_transition_pool
                    .get(&2, state)
                    .unwrap_infallible()
                    .is_none(),
                "The transition should not exist in the pool"
            );
        }),
    };
    let rebond_attester = TransactionTestCase {
        input: genesis_attester.create_plain_message::<AttesterIncentives<S, MockDaSpec>>(
            CallMessage::RegisterAttester(genesis_attester.bond),
        ),
        assert: Box::new(move |result, state| {
            assert!(result.events.iter().any(|event| matches!(
                event,
                TestRuntimeEvent::attester_incentives(Event::RegisteredAttester { .. })
            )));
            assert_eq!(
                AttesterIncentives::<S, MockDaSpec>::default()
                    .get_attester_bond_amount(genesis_attester_address, state)
                    .unwrap_infallible()
                    .value,
                TEST_DEFAULT_USER_STAKE,
            );
        }),
    };
    let invalid_post_state_root_is_challengeable = TransactionTestCase {
        input: genesis_attester
            .test_process_attestation(Err(TestAttestationMessageError::InvalidPostStateRoot)),
        assert: Box::new(move |result, state| {
            assert!(result.events.iter().any(|event| matches!(
                event,
                TestRuntimeEvent::attester_incentives(Event::UserSlashed { .. })
            )));
            // Assert that the attester was slashed
            assert_eq!(
                AttesterIncentives::<S, MockDaSpec>::default()
                    .get_attester_bond_amount(genesis_attester_address, state)
                    .unwrap_infallible()
                    .value,
                0,
            );
            // The attestation should be part of the challengeable set and its associated value should be the BOND_AMOUNT
            assert_eq!(
                AttesterIncentives::<S, MockDaSpec>::default()
                    .bad_transition_pool
                    .get(&2, state)
                    .unwrap_infallible(),
                Some(genesis_attester_bond),
                "The transition should exist in the pool"
            );
        }),
    };

    runner
        .advance_slots(2)
        .execute_transaction(invalid_bond_proof_no_slash)
        .execute_transaction(valid_attestation)
        .execute_transaction(invalid_initial_state_slashed)
        .execute_transaction(rebond_attester)
        .execute_transaction(invalid_post_state_root_is_challengeable);
}
