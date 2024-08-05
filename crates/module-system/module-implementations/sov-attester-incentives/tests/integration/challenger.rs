use std::convert::Infallible;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use sov_attester_incentives::{CallMessage, Event, Role, SlashingReason};
use sov_bank::Amount;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::GasMeter;
use sov_test_utils::generators::attester_incentive::framework::TestChallengeGenerator;
use sov_test_utils::generators::attester_incentive::{
    TestAttestationMessageError, TestChallengeMessageError,
};
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{
    AsUser, BondedTestChallenger, SlotTestCase, TestAttester, TxTestCase, TEST_DEFAULT_USER_STAKE,
    TEST_ROLLUP_FINALITY_PERIOD,
};

use crate::helpers::{setup, TestAttesterIncentives, RT, S};

/// Helper that sets up a configuration where:
/// - the challenger is bonded and
/// - there is a wrong attestation to challenge in the first slot.
fn setup_with_wrong_attestation() -> (
    TestRunner<RT, S>,
    TestAttester<S>,
    BondedTestChallenger<S>,
    Amount,
) {
    let (mut runner, mut genesis_attester, mut genesis_challenger, _) = setup();

    let genesis_attester_address = genesis_attester.user_info.address();
    let genesis_attester_bond = genesis_attester.bond;

    let genesis_challenger_address = genesis_challenger.user_info.address();
    let genesis_challenger_bond = TEST_DEFAULT_USER_STAKE;

    let expected_challenger_balance =
        Arc::new(AtomicU64::new(genesis_challenger.user_info.balance()));
    let expected_challenger_balance_2 = expected_challenger_balance.clone();
    let expected_challenger_balance_3 = expected_challenger_balance.clone();

    let expected_attester_balance = Arc::new(AtomicU64::new(genesis_attester.user_info.balance()));
    let expected_attester_balance_2 = expected_attester_balance.clone();

    // Start by bonding the challenger and checking he is correctly bounded
    let mut bond_challenger = vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<RT, _, _>::applied_with_hook(
            genesis_challenger.create_plain_message::<TestAttesterIncentives>(
                CallMessage::BondChallenger(genesis_challenger_bond),
            ),
            Box::new(move |state| {
                assert_eq!(
                    TestAttesterIncentives::default()
                        .get_bond_amount(genesis_challenger_address, Role::Challenger, state)
                        .unwrap_infallible()
                        .value,
                    genesis_challenger_bond,
                    "Challenger not bonded"
                );

                // Update the challenger balance (because they consumed some gas and bonded)
                expected_challenger_balance.fetch_sub(
                    state.inner().gas_used_value(),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }),
        ),
    ])
    .with_end_slot_hook(Box::new(move |state| {
        // Check that the challenger has the correct balance
        assert_eq!(
            TestRunner::<RT, S>::bank_gas_balance(&genesis_challenger_address, state),
            Some(
                expected_challenger_balance_2.load(std::sync::atomic::Ordering::SeqCst)
                    - genesis_challenger_bond
            ),
            "The attester should have the correct bond amount from genesis"
        );
    }))];

    bond_challenger.append(
        // Then execute empty transactions to reach finality
        &mut SlotTestCase::empty_slots(TEST_ROLLUP_FINALITY_PERIOD as usize),
    );

    runner.execute_slots(bond_challenger);

    genesis_challenger.user_info.available_balance =
        expected_challenger_balance_3.load(std::sync::atomic::Ordering::SeqCst);

    let bonded_challenger =
        BondedTestChallenger::from_challenger(genesis_challenger, genesis_challenger_bond);

    // Then attest the first slot with a wrong attestation
    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<RT, _, _>::applied_with_hook(
            genesis_attester
                .test_process_attestation(Err(TestAttestationMessageError::InvalidPostStateRoot)),
            Box::new(move |state| {
                // Check that the working set has emitted a slashed event
                assert!(state.inner().events().iter().any(|event| matches!(
                    event.downcast_ref::<Event<S>>(),
                    Some(Event::UserSlashed { .. })
                )));

                // Check that the attester was slashed
                assert_eq!(
                    TestAttesterIncentives::default()
                        .get_bond_amount(genesis_attester_address, Role::Attester, state)
                        .unwrap_infallible()
                        .value,
                    0,
                );

                // Check that the transition was added to the challengeable set
                assert_eq!(
                    TestAttesterIncentives::default()
                        .bad_transition_pool
                        // The attestation is added to the challengeable set at the slot of the attestation.
                        // Since the attestation is invalid, the slot to attest is the same as the slot of the faulty attestation.
                        // (the slot to attest is not increaed when the attestation is faulty)
                        .get(&(1), state)
                        .unwrap_infallible(),
                    Some(genesis_attester_bond),
                    "The failed attestation should have been added to the challengeable set"
                );

                expected_attester_balance.fetch_sub(
                    state.inner().gas_used_value(),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }),
        ),
    ])
    .with_end_slot_hook(Box::new(move |state| {
        // Check that the attester has the correct balance
        assert_eq!(
            TestRunner::<RT, S>::bank_gas_balance(&genesis_attester_address, state),
            Some(expected_attester_balance_2.load(std::sync::atomic::Ordering::SeqCst)),
            "The attester should have the correct bond amount from genesis"
        );
    }))]);

    (
        runner,
        genesis_attester,
        bonded_challenger,
        genesis_attester_bond,
    )
}

/// Test that given an invalid transition, a challenger can successfully challenge it and get rewarded
/// This tests the happy path of challenge processing.
#[test]
fn test_valid_challenge() -> Result<(), Infallible> {
    let (mut runner, _, bonded_challenger, expected_reward) = setup_with_wrong_attestation();

    let bonded_challenger_address = bonded_challenger.user_info.address();
    let bonded_challenger_balance = bonded_challenger.user_info.balance();

    let bonded_challenger_balance_ref = Arc::new(AtomicU64::new(bonded_challenger_balance));
    let bonded_challenger_balance_ref_2 = bonded_challenger_balance_ref.clone();

    // Then challenge the wrongly attested slot.
    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<RT, _, _>::applied_with_hook(
            bonded_challenger.test_process_challenge_at_slot(Ok(()), 1),
            Box::new(move |state| {
                // Check that the challenge set is now empty
                assert_eq!(
                    TestAttesterIncentives::default()
                        .bad_transition_pool
                        .get(&(1), state)
                        .unwrap_infallible(),
                    None,
                    "The transition should have disappeared from the pool"
                );

                // Check that a reward event has been emitted
                assert!(
                    state.inner().events().iter().any(|event| {
                        event.downcast_ref::<Event<S>>()
                            == Some(&Event::ProcessedValidProof {
                                challenger: bonded_challenger_address,
                            })
                    }),
                    "There was no reward event emitted"
                );

                bonded_challenger_balance_ref.fetch_sub(
                    state.inner().gas_used_value(),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }),
        ),
    ])
    .with_end_slot_hook(Box::new(move |state| {
        // Check that the challenger was rewarded
        let reward = TestAttesterIncentives::default()
            .burn_rate()
            .apply(expected_reward);

        assert_eq!(
            TestRunner::<RT, S>::bank_gas_balance(&bonded_challenger_address, state),
            Some(
                bonded_challenger_balance_ref_2.load(std::sync::atomic::Ordering::SeqCst) + reward
            ),
            "The challenger has not been rewarded the correct amount"
        );
    }))]);

    Ok(())
}

fn test_invalid_challenge_helper(
    error_type: TestChallengeMessageError,
    slashing_reason: SlashingReason,
) {
    let (mut runner, _, bonded_challenger, expected_reward) = setup_with_wrong_attestation();

    let bonded_challenger_address = bonded_challenger.user_info.address();
    let bonded_challenger_balance = bonded_challenger.user_info.balance();

    let bonded_challenger_balance_ref = Arc::new(AtomicU64::new(bonded_challenger_balance));
    let bonded_challenger_balance_ref_2 = bonded_challenger_balance_ref.clone();

    // Then challenge the wrongly attested slot.
    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<RT, _, _>::applied_with_hook(
            bonded_challenger.test_process_challenge_at_slot(Err(error_type), 1),
            Box::new(move |state| {
                // Check that a slashing event has been emitted and that it is the correct one
                assert!(
                    state.inner().events().iter().any(|event| {
                        let event = event.downcast_ref::<Event<S>>();
                        if matches!(event, Some(Event::UserSlashed { .. })) {
                            assert_eq!(
                                event,
                                Some(&Event::UserSlashed {
                                    address: bonded_challenger_address,
                                    reason: slashing_reason,
                                }),
                                "The correct slashing event was not emitted"
                            );

                            return true;
                        }

                        false
                    }),
                    "No slashing event were emitted"
                );

                // Check that the challenger was slashed
                assert_eq!(
                    TestAttesterIncentives::default()
                        .bonded_challengers
                        .get(&bonded_challenger_address, state)
                        .unwrap_infallible(),
                    None,
                    "The challenger was not removed from the bonded challengers set"
                );

                bonded_challenger_balance_ref.fetch_sub(
                    state.inner().gas_used_value(),
                    std::sync::atomic::Ordering::SeqCst,
                );

                // Check that the challenge set is *not* empty
                assert_eq!(
                    TestAttesterIncentives::default()
                        .bad_transition_pool
                        .get(&(1), state)
                        .unwrap_infallible(),
                    Some(expected_reward),
                    "The transition should *not* have disappeared from the pool"
                );
            }),
        ),
    ])
    .with_end_slot_hook(Box::new(move |state| {
        // Check that the challenger was not rewarded

        assert_eq!(
            TestRunner::<RT, S>::bank_gas_balance(&bonded_challenger_address, state),
            Some(bonded_challenger_balance_ref_2.load(std::sync::atomic::Ordering::SeqCst)),
            "The challenger balance is not correct"
        );
    }))]);
}

#[test]
fn test_invalid_challenge_initial_state_root() {
    test_invalid_challenge_helper(
        TestChallengeMessageError::InvalidInitialStateRoot,
        SlashingReason::InvalidInitialHash,
    );
}

#[test]
fn test_invalid_challenge_transition() {
    test_invalid_challenge_helper(
        TestChallengeMessageError::InvalidTransition,
        SlashingReason::TransitionInvalid,
    );
}

#[test]
fn test_invalid_challenge_proof() {
    test_invalid_challenge_helper(
        TestChallengeMessageError::InvalidChallengeProof,
        SlashingReason::InvalidProofOutputs,
    );
}
