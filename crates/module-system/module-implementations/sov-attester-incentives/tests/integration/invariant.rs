use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use sov_attester_incentives::{Event, ProcessAttestationErrors};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::Error::ModuleError;
use sov_modules_api::{Spec, StateAccessorError};
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{TestAttester, TransactionTestCase, TEST_ROLLUP_FINALITY_PERIOD};

use crate::helpers::{setup, TestAttesterIncentives, __GeneratedRuntimeInternalsEvent, RT, S};

/// Sets up the invariant tests by executing empty slots and attesting up to `FINALITY_PERIOD + 1`. The maximum attested height is
/// equal to the `FINALITY_PERIOD + 1` at the end of the setup. Returns the test runner, genesis attester and the maximum attested height.
fn setup_invariant_tests() -> (TestRunner<RT, S>, TestAttester<S>, u64) {
    let (mut runner, mut genesis_attester, _, _) = setup();

    let genesis_attester_address = genesis_attester.user_info.address();

    runner.advance_slots(TEST_ROLLUP_FINALITY_PERIOD as usize);

    let max_attested_height_ref = Arc::new(AtomicU64::new(1));

    // Increase the max attested height by attesting to up to the finality period + 1.
    for i in 1..=TEST_ROLLUP_FINALITY_PERIOD + 1 {
        let max_attested_height_ref_loop = max_attested_height_ref.clone();

        runner.execute_transaction(TransactionTestCase {
            input: genesis_attester.test_process_attestation(Ok(())),
            assert: Box::new(move |result, state| {
                // Check that we emitted a `ProcessedValidAttestation` event.
                assert!(result.events.iter().any(|event| matches!(
                    event,
                    __GeneratedRuntimeInternalsEvent::attester_incentives(
                        Event::ProcessedValidAttestation { attester }
                    ) if *attester == genesis_attester_address
                )));
                // Ensure that the `MAX_ATTESTED_HEIGHT` increases by 1.
                let max_attested_height = TestAttesterIncentives::default()
                    .maximum_attested_height
                    .get(state)
                    .unwrap_infallible()
                    .unwrap();
                assert_eq!(
                    max_attested_height,
                    max_attested_height_ref_loop.load(Ordering::SeqCst),
                    "The max attested height should have increased by 1. Slot height {i}"
                );

                max_attested_height_ref_loop.fetch_add(1, Ordering::SeqCst);
            }),
        });
    }

    (runner, genesis_attester, TEST_ROLLUP_FINALITY_PERIOD + 1)
}

/// The attesters need to publish attestestations for slots above `MAX_ATTESTED_HEIGHT - ROLLUP_FINALITY_PERIOD`.
#[test]
fn test_cannot_attest_below_max_attested_height() {
    let (mut runner, genesis_attester, expected_max_attested_height) = setup_invariant_tests();

    // Now try to attest to a block at height 1. This is stricly below `MAX_ATTESTED_HEIGHT - TEST_ROLLUP_FINALITY_PERIOD`.
    // The transaction should revert.
    runner.execute_transaction(TransactionTestCase {
        input: genesis_attester.test_process_attestation_at_slot(Ok(()), 1),
        assert: Box::new(move |result, state| {
            match &result.outcome {
                sov_modules_api::TxEffect::Reverted(reason) => {
                    assert_eq!(
                        reason,
                        &ModuleError(ProcessAttestationErrors::<StateAccessorError<<S as Spec>::Gas>>::InvalidTransitionInvariant.into()),
                        "Transaction reverted, but with unexpected reason"
                    );
                },
                unexpected => panic!("Expected transaction to revert, but got: {:?}", unexpected),
            };

            let max_attested_height = TestAttesterIncentives::default()
                .maximum_attested_height
                .get(state)
                .unwrap_infallible()
                .unwrap();
            let finality = TestAttesterIncentives::default()
                .rollup_finality_period
                .get(state)
                .unwrap_infallible()
                .unwrap();

            assert_eq!(max_attested_height, expected_max_attested_height, "Sanity check failed: the max attested height should be {expected_max_attested_height}, but it is {max_attested_height}");

            assert!(
                max_attested_height > finality,
                "The difference between the max attested height (value: {max_attested_height})
         and the finality period (value: {finality}) should be greater than 1"
            );
        }),
    });
}

/// The attesters need to publish attestestations for slots below `MAX_ATTESTED_HEIGHT + 1`.
#[test]
fn test_cannot_attest_above_max_attested_height_plus_one() {
    let (mut runner, genesis_attester, expected_max_attested_height) = setup_invariant_tests();

    // Now try to attest to a block at `MAX_ATTESTED_HEIGHT + 2`. This is stricly above `MAX_ATTESTED_HEIGHT + 1`.
    // The transaction should revert.
    runner.execute_transaction(TransactionTestCase {
        input: genesis_attester.test_process_attestation_at_slot(Ok(()), 1),
        assert: Box::new(move |result, state| {
            match &result.outcome {
                sov_modules_api::TxEffect::Reverted(reason) => {
                    assert_eq!(
                        reason,
                        &ModuleError(ProcessAttestationErrors::<StateAccessorError<<S as Spec>::Gas>>::InvalidTransitionInvariant.into()),
                        "Transaction reverted, but with unexpected reason"
                    );
                },
                unexpected => panic!("Expected transaction to revert, but got: {:?}", unexpected),
            };
            // Ensure that the `MAX_ATTESTED_HEIGHT` increases by 1.
            let max_attested_height = TestAttesterIncentives::default()
                .maximum_attested_height
                .get(state)
                .unwrap_infallible()
                .unwrap();
            assert_eq!(max_attested_height, expected_max_attested_height, "Sanity check failed: the max attested height should be {expected_max_attested_height}, but it is {max_attested_height}");
        }),
    });
}

/// Test that the attesters can publish attestations for slots within the range `MAX_ATTESTED_HEIGHT - ROLLUP_FINALITY_PERIOD` to `MAX_ATTESTED_HEIGHT + 1`.
/// If attesters publish attestations in the range `MAX_ATTESTED_HEIGHT - ROLLUP_FINALITY_PERIOD + 1` to `MAX_ATTESTED_HEIGHT`, the attestations are valid but the max attested height is not updated.
#[test]
fn test_can_attest_within_allowed_range() {
    let (mut runner, genesis_attester, expected_max_attested_height) = setup_invariant_tests();

    let genesis_attester_address = genesis_attester.user_info.address();

    // Now try to attest every slot between `MAX_ATTESTED_HEIGHT - FINALITY_PERIOD + 1` and `MAX_ATTESTED_HEIGHT`. Check that the attestations are valid but the sequence is not rewarded.
    for i in 0..TEST_ROLLUP_FINALITY_PERIOD {
        runner.execute_transaction(TransactionTestCase {
            input: genesis_attester
                .test_process_attestation_at_slot(Ok(()), expected_max_attested_height - i),
            assert: Box::new(move |result, state| {
                // Check that we emitted a `ProcessedValidAttestation` event.
                assert!(result.events.iter().any(|event| matches!(
                    event,
                    __GeneratedRuntimeInternalsEvent::attester_incentives(
                        Event::ProcessedValidAttestation { attester }
                    ) if *attester == genesis_attester_address
                )));

                // Ensure that the `MAX_ATTESTED_HEIGHT` does not increase.
                let max_attested_height = TestAttesterIncentives::default()
                    .maximum_attested_height
                    .get(state)
                    .unwrap_infallible()
                    .unwrap();
                assert_eq!(
                    max_attested_height, expected_max_attested_height,
                    "The max attested height should not have changed. Slot height {i}"
                );
            }),
        });
    }
}
