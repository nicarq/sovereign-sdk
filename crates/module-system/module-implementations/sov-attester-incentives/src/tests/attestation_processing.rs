use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use sov_mock_da::MockDaSpec;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{Error, GasMeter, StateCheckpoint};
use sov_test_utils::generators::attester_incentive::TestAttestationMessageError;
use sov_test_utils::runtime::sov_attester_incentives::{
    AttesterIncentives, CallMessage, Event, Role,
};
use sov_test_utils::{AsUser, SlotTestCase, TxTestCase, TEST_DEFAULT_USER_STAKE};

use super::helpers_framework::TestAttesterIncentives;
use crate::tests::helpers_framework::{setup, RT, S};
use crate::AttesterIncentiveErrors;

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

    // We run a test with 5 slots (plus genesis). The first slot is empty and is executed in the setup function.
    // The second slot is also empty. The third and fourth slots attest to the first two empty slots. The last
    // slot attest to the first slot that contains a transaction. This allows us to test that gas metering is done correctly.
    runner.execute_slots(vec![
        // Run an empty slot
        SlotTestCase::empty(),
        // Attest to the first slot. Check that a ProcessedValidAttestation attestation
        // event is emitted and do necessary accounting to check the attester's balance later
        SlotTestCase::from_rewarded_batch(vec![TxTestCase::<RT, _, _>::applied_with_hook(
            genesis_attester.test_process_attestation(Ok(())),
            Box::new(move |ws| {
                // Do accounting for the attester's balance
                {
                    // The attester's balance should be decremented by the gas used
                    expected_balance.fetch_sub(
                        ws.inner().gas_used_value(),
                        std::sync::atomic::Ordering::SeqCst,
                    );
                    // We know that attester will attest to this slot later, so he'll get back some of his gas at that point.
                    expected_balance.fetch_add(
                        AttesterIncentives::<S, MockDaSpec>::default()
                            .burn_rate()
                            .apply(ws.inner().gas_used_value()),
                        std::sync::atomic::Ordering::SeqCst,
                    );
                }

                // Check that the attestation succeeded
                assert!(ws.inner().events().iter().any(|event| matches!(
                    event.downcast_ref::<Event<S>>(),
                    Some(Event::ProcessedValidAttestation { .. })
                )));
            }),
        )]),
        SlotTestCase::from_rewarded_batch(vec![TxTestCase::<RT, _, _>::applied_with_hook(
            genesis_attester.test_process_attestation(Ok(())),
            Box::new(move |ws| {
                // Check that the attestation succeeded
                assert!(ws.inner().events().iter().any(|event| matches!(
                    event.downcast_ref::<Event<S>>(),
                    Some(Event::ProcessedValidAttestation { .. })
                )));
                // Account for the gas used to send the attestation. We never attest to the current slot, so we don't add anything back.
                expected_balance_ref_1.fetch_sub(
                    ws.inner().gas_used_value(),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }),
        )]),
        SlotTestCase::from_rewarded_batch(vec![TxTestCase::<RT, _, _>::applied_with_hook(
            genesis_attester.test_process_attestation(Ok(())),
            Box::new(move |ws| {
                // Check that the attestation succeeded
                assert!(ws.inner().events().iter().any(|event| matches!(
                    event.downcast_ref::<Event<S>>(),
                    Some(Event::ProcessedValidAttestation { .. })
                )));
                // Account for the gas used to send the attestation. We never attest to the current slot, so we don't add anything back.
                expected_balance_ref_2.fetch_sub(
                    ws.inner().gas_used_value(),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }),
        )])
        .with_end_slot_hook(Box::new(move |state_checkpoint| {
            assert_eq!(
                sov_bank::Bank::<S>::default()
                    .get_balance_of(
                        &genesis_attester_address,
                        sov_bank::GAS_TOKEN_ID,
                        state_checkpoint
                    )
                    .unwrap_infallible()
                    .unwrap(),
                expected_balance_ref_3.load(std::sync::atomic::Ordering::SeqCst)
            );

            // Check that the attester still has their full bond
            assert_eq!(
                AttesterIncentives::<S, MockDaSpec>::default()
                    .get_bond_amount(genesis_attester_address, Role::Attester, state_checkpoint)
                    .unwrap_infallible()
                    .value,
                genesis_attester_bond,
            );
        })),
    ]);
}

#[test]
fn test_burn_on_invalid_attestation() {
    let (mut runner, mut genesis_attester, _, _) = setup();

    let genesis_attester_address = genesis_attester.user_info.address();
    let genesis_attester_bond = genesis_attester.bond;

    runner.execute_slots(vec![
        // Run any empty slot, and check that the attester has the correct bond amount from genesis
        SlotTestCase::<_, TestAttesterIncentives, _>::empty().with_end_slot_hook(Box::new(
            move |ws: &mut StateCheckpoint<S>| {
                // Assert that genesis yielded the expected bond amount
                assert_eq!(
                    AttesterIncentives::<S, MockDaSpec>::default()
                        .bonded_attesters
                        .get(&genesis_attester_address, ws)
                        .unwrap_infallible()
                        .unwrap_or_default(),
                    genesis_attester_bond,
                );
            },
        )),
        // Run an empty slot
        SlotTestCase::empty(),
        SlotTestCase::from_rewarded_batch(vec![TxTestCase::reverted(
            genesis_attester
                .test_process_attestation(Err(TestAttestationMessageError::InvalidProofOfBond)),
            Error::ModuleError(AttesterIncentiveErrors::InvalidBondingProof.into()),
        )])
        .with_end_slot_hook(Box::new(move |state| {
            // Assert that the attester was not slashed
            assert_eq!(
                AttesterIncentives::<S, MockDaSpec>::default()
                    .get_bond_amount(genesis_attester_address, Role::Attester, state)
                    .unwrap_infallible()
                    .value,
                genesis_attester_bond,
            );
        })),
        SlotTestCase::from_rewarded_batch(vec![TxTestCase::<RT, _, _>::applied_with_hook(
            genesis_attester.test_process_attestation(Ok(())),
            Box::new(|state| {
                // Check that the attestation succeeded
                assert!(state.inner().events().iter().any(|event| matches!(
                    event.downcast_ref::<Event<S>>(),
                    Some(Event::ProcessedValidAttestation { .. })
                )));
            }),
        )]),
        SlotTestCase::from_rewarded_batch(vec![TxTestCase::<RT, _, _>::applied_with_hook(
            genesis_attester.test_process_attestation(Err(
                TestAttestationMessageError::InvalidInitialStateRoot,
            )),
            Box::new(move |state| {
                // Check that the attestation resulted in slashing
                assert!(state.inner().events().iter().any(|event| matches!(
                    event.downcast_ref::<Event<S>>(),
                    Some(Event::UserSlashed { .. })
                )));
                // Assert that the attester was slashed
                assert_eq!(
                    AttesterIncentives::<S, MockDaSpec>::default()
                        .get_bond_amount(genesis_attester_address, Role::Attester, state)
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
        )]),
    ]);

    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<RT, _, _>::applied_with_hook(
            genesis_attester.create_plain_message::<AttesterIncentives<S, MockDaSpec>>(
                CallMessage::BondAttester(genesis_attester.bond),
            ),
            Box::new(move |state| {
                assert!(state.inner().events().iter().any(|event| matches!(
                    event.downcast_ref::<Event<S>>(),
                    Some(Event::BondedAttester { .. })
                )));
                assert_eq!(
                    AttesterIncentives::<S, MockDaSpec>::default()
                        .get_bond_amount(genesis_attester_address, Role::Attester, state)
                        .unwrap_infallible()
                        .value,
                    TEST_DEFAULT_USER_STAKE,
                );
            }),
        ),
    ])]);

    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<RT, _, _>::applied_with_hook(
            genesis_attester
                .test_process_attestation(Err(TestAttestationMessageError::InvalidPostStateRoot)),
            Box::new(move |state| {
                // Check that the attestation resulted in slashing
                assert!(state.inner().events().iter().any(|event| matches!(
                    event.downcast_ref::<Event<S>>(),
                    Some(Event::UserSlashed { .. })
                )));
                // Assert that the attester was slashed
                assert_eq!(
                    AttesterIncentives::<S, MockDaSpec>::default()
                        .get_bond_amount(genesis_attester_address, Role::Attester, state)
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
        ),
    ])]);
}
