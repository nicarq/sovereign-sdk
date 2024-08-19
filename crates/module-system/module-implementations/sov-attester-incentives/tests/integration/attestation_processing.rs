use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use sov_attester_incentives::ProcessAttestationErrors;
use sov_mock_da::MockDaSpec;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{Error, Spec, StateAccessorError, TxEffect};
use sov_test_utils::generators::attester_incentive::TestAttestationMessageError;
use sov_test_utils::runtime::sov_attester_incentives::{AttesterIncentives, CallMessage, Event};
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{AsUser, TransactionTestCase, TEST_DEFAULT_USER_STAKE};

use super::helpers::{setup, TestRuntimeEvent, RT, S};

/// Start by testing the positive case where the attestations are valid. We check that...
/// valid attestations are processed correctly
/// attesters are rewarded as expected
#[test]
fn test_process_valid_attestation() {
    let (mut runner, mut genesis_attester, _, _) = setup();

    let genesis_attester_address = genesis_attester.user_info.address();
    let genesis_attester_bond = genesis_attester.bond;
    let genesis_attester_balance = genesis_attester.user_info.available_balance;

    // We use an arc of an atomic to do accounting for the expected balance.
    // because of limitations in rusts capture rules, we need a bunch of clones
    // of this arc ahead of time
    let expected_balance = Arc::new(AtomicU64::new(genesis_attester_balance));
    let expected_balance_ref_1 = expected_balance.clone();
    let expected_balance_ref_2 = expected_balance.clone();
    let expected_balance_ref_3 = expected_balance.clone();

    let attest_slot_1 = TransactionTestCase {
        input: genesis_attester.test_process_attestation(Ok(())),
        assert: Box::new(move |result, _state| {
            // Do accounting for the attester's balance
            {
                // The attester's balance should be decremented by the gas used
                expected_balance.fetch_sub(result.gas_used, std::sync::atomic::Ordering::SeqCst);
                // We know that attester will attest to this slot later, so he'll get back some of his gas at that point.
                expected_balance.fetch_add(
                    AttesterIncentives::<S, MockDaSpec>::default()
                        .burn_rate()
                        .apply(result.gas_used),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }
            // Check that the attestation succeeded
            assert!(result.events.iter().any(|event| matches!(
                event,
                TestRuntimeEvent::attester_incentives(Event::ProcessedValidAttestation { .. })
            )));
        }),
    };
    let attest_slot_2 = TransactionTestCase {
        input: genesis_attester.test_process_attestation(Ok(())),
        assert: Box::new(move |result, _state| {
            // Check that the attestation succeeded
            assert!(result.events.iter().any(|event| matches!(
                event,
                TestRuntimeEvent::attester_incentives(Event::ProcessedValidAttestation { .. })
            )));
            // Account for the gas used to send the attestation. We never attest to the current slot, so we don't add anything back.
            expected_balance_ref_1.fetch_sub(result.gas_used, std::sync::atomic::Ordering::SeqCst);
        }),
    };
    let attest_to_first_attestation = TransactionTestCase {
        input: genesis_attester.test_process_attestation(Ok(())),
        assert: Box::new(move |result, state| {
            // Check that the attestation succeeded
            assert!(result.events.iter().any(|event| matches!(
                event,
                TestRuntimeEvent::attester_incentives(Event::ProcessedValidAttestation { .. })
            )));
            // Account for the gas used to send the attestation. We never attest to the current slot, so we don't add anything back.
            expected_balance_ref_2.fetch_sub(result.gas_used, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(
                TestRunner::<RT, S>::bank_gas_balance(&genesis_attester_address, state),
                Some(expected_balance_ref_3.load(std::sync::atomic::Ordering::SeqCst))
            );
            // Check that the attester still has their full bond
            assert_eq!(
                AttesterIncentives::<S, MockDaSpec>::default()
                    .get_attester_bond_amount(genesis_attester_address, state)
                    .unwrap_infallible()
                    .value,
                genesis_attester_bond,
            );
        }),
    };

    // We run a test with 5 slots (plus genesis).
    // The first two slots are empty. The third and fourth slots attest to the first two empty slots. The last
    // slot attest to the first slot that contains a transaction. This allows us to test that gas metering is done correctly.
    runner
        .advance_slots(2)
        .execute_transaction(attest_slot_1)
        .execute_transaction(attest_slot_2)
        .execute_transaction(attest_to_first_attestation);
}

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
