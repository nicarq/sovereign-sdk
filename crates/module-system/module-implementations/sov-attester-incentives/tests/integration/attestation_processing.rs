use sov_mock_da::MockDaSpec;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{InvalidProofError, ProofOutcome};
use sov_state::jmt::RootHash;
use sov_state::StorageRoot;
use sov_test_utils::runtime::sov_attester_incentives::{AttesterIncentives, CallMessage, Event};
use sov_test_utils::{
    AsUser, ProofTestCase, ProofType, TransactionTestCase, TEST_DEFAULT_USER_STAKE,
};

use super::helpers::{setup, TestRuntimeEvent, S};
use crate::helpers::{
    build_proof, consume_gas_tx_for_signer, create_test_case, make_attestation_blob,
    TestAttesterIncentives,
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

#[test]
fn test_burn_on_invalid_attestation() {
    let (mut runner, genesis_attester, _, other_user) = setup();

    for _ in 0..5 {
        // Crate a couple of batches.
        runner.execute(consume_gas_tx_for_signer(&other_user), None);
    }

    let attester_addr = genesis_attester.user_info.address();
    let attester_bond = genesis_attester.bond;

    let invalid_bond_proof_no_slash = {
        let mut attestation_proof = runner
            .query_state(|state| build_proof(state, 1, &attester_addr))
            .unwrap();

        attestation_proof.proof_of_bond.claimed_transition_num = 2;

        ProofTestCase {
            input: ProofType::Inline(make_attestation_blob(attestation_proof)),
            override_sequencer: None,
            assert: Box::new(move |result, state| {
                assert_eq!(
                    result.outcome.as_ref().unwrap().outcome,
                    ProofOutcome::Invalid(InvalidProofError::PreconditionNotMet(
                        "Invalid bonding proof".to_string()
                    ))
                );

                assert_eq!(
                    TestAttesterIncentives::default()
                        .bonded_attesters
                        .get(&attester_addr, state)
                        .unwrap(),
                    Some(attester_bond),
                    "Bonded amount should not have changed"
                );
            }),
        }
    };

    let valid_attestation = {
        let attestation_proof_2 = runner
            .query_state(|state| build_proof(state, 1, &attester_addr))
            .unwrap();

        create_test_case(
            genesis_attester.clone(),
            make_attestation_blob(attestation_proof_2),
        )
    };

    let invalid_initial_state_slashed = {
        let mut attestation_proof = runner
            .query_state(|state| build_proof(state, 1, &attester_addr))
            .unwrap();

        attestation_proof.initial_state_root =
            StorageRoot::new(RootHash([255; 32]), RootHash([255; 32]));

        ProofTestCase {
            input: ProofType::Inline(make_attestation_blob(attestation_proof)),
            override_sequencer: None,
            assert: Box::new(move |_result, state| {
                // TODO: #1292
                // assert_matches!(
                //    result.outcome.unwrap().outcome,
                //    ProofOutcome::Invalid(InvalidProofError::PreconditionNotMet(_))
                //);

                assert!(TestAttesterIncentives::default()
                    .bonded_attesters
                    .get(&attester_addr, state)
                    .unwrap()
                    .is_none());

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
        }
    };

    let rebond_attester = {
        TransactionTestCase {
            input: genesis_attester.create_plain_message::<AttesterIncentives<S, MockDaSpec>>(
                CallMessage::RegisterAttester(attester_bond),
            ),
            assert: Box::new(move |result, state| {
                assert!(result.events.iter().any(|event| matches!(
                    event,
                    TestRuntimeEvent::attester_incentives(Event::RegisteredAttester { .. })
                )));
                assert_eq!(
                    AttesterIncentives::<S, MockDaSpec>::default()
                        .get_attester_bond_amount(&attester_addr, state)
                        .unwrap_infallible()
                        .value,
                    TEST_DEFAULT_USER_STAKE,
                );
            }),
        }
    };

    let invalid_post_state_root_is_challengeable = {
        let mut attestation_proof = runner
            .query_state(|state| build_proof(state, 2, &attester_addr))
            .unwrap();

        attestation_proof.post_state_root =
            StorageRoot::new(RootHash([255; 32]), RootHash([255; 32]));

        ProofTestCase {
            input: ProofType::Inline(make_attestation_blob(attestation_proof)),
            override_sequencer: None,
            assert: Box::new(move |_result, state| {
                // TODO #1292:
                // assert_matches!(
                //    result.outcome.unwrap().outcome,
                //    ProofOutcome::Invalid(InvalidProofError::PreconditionNotMet(_))
                // );

                // TODO #1292: check rewards.

                assert!(TestAttesterIncentives::default()
                    .bonded_attesters
                    .get(&attester_addr, state)
                    .unwrap()
                    .is_none(),);

                // The attestation should be part of the challengeable set and its associated value should be the BOND_AMOUNT
                assert_eq!(
                    AttesterIncentives::<S, MockDaSpec>::default()
                        .bad_transition_pool
                        .get(&2, state)
                        .unwrap_infallible(),
                    Some(attester_bond),
                    "The transition should exist in the pool"
                );
            }),
        }
    };

    runner
        .execute_proof::<TestAttesterIncentives>(invalid_bond_proof_no_slash)
        .execute_proof::<TestAttesterIncentives>(valid_attestation)
        .execute_proof::<TestAttesterIncentives>(invalid_initial_state_slashed)
        .execute_transaction(rebond_attester)
        .execute_proof::<TestAttesterIncentives>(invalid_post_state_root_is_challengeable);
}
