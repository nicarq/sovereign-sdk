use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use sov_attester_incentives::{AttesterIncentiveErrors, Event, UnbondingInfo};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::Error::ModuleError;
use sov_modules_api::{GasMeter, Spec, StateAccessorError};
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{
    AsUser, SlotTestCase, TestAttester, TxTestCase, TEST_LIGHT_CLIENT_FINALIZED_HEIGHT,
    TEST_ROLLUP_FINALITY_PERIOD,
};

use crate::helpers::{setup, TestAttesterIncentives, RT, S};

const INIT_BONDING_HEIGHT: u64 = TEST_LIGHT_CLIENT_FINALIZED_HEIGHT;

/// Checks that the attester is bonded and starts the unbonding process.
/// Returns the gas consumed by the attester when submitting the unbonding transaction.
fn check_attester_bonded_and_start_unbond(
    runner: &mut TestRunner<RT, S>,
    attester: &TestAttester<S>,
) -> u64 {
    let attester_address = attester.user_info.address();
    let attester_bond = attester.bond;
    let attester_balance = attester.user_info.balance();
    let gas_consumed_attester_ref_1 = Arc::new(AtomicU64::new(0));
    let gas_consumed_attester_ref_2 = gas_consumed_attester_ref_1.clone();

    runner.execute_slots(vec![
        // Start by checking the attester balance and bond.
        SlotTestCase::empty().with_end_slot_hook(Box::new(move |state| {
            // Check that the attester account is bonded
            assert_eq!(
                TestAttesterIncentives::default()
                    .bonded_attesters
                    .get(&attester_address, state)
                    .unwrap(),
                Some(attester_bond),
                "The genesis attester should be bonded"
            );

            // Check the balance of the attester is equal to the free balance
            assert_eq!(
                TestRunner::<RT, S>::bank_gas_balance(&attester_address, state),
                Some(attester_balance),
                "The balance of the attester should be equal to the free balance"
            );
        })),
        // Initiate unbonding
        SlotTestCase::from_rewarded_batch(vec![TxTestCase::<RT, _, _>::applied_with_hook(
            attester.create_plain_message::<TestAttesterIncentives>(
                sov_attester_incentives::CallMessage::BeginUnbondingAttester,
            ),
            Box::new(move |state| {
                // Check that the attester is part of the unbonding set
                assert_eq!(
                    TestAttesterIncentives::default()
                        .unbonding_attesters
                        .get(&attester_address, state)
                        .unwrap(),
                    Some(UnbondingInfo {
                        unbonding_initiated_height: INIT_BONDING_HEIGHT,
                        amount: attester_bond
                    }),
                );

                gas_consumed_attester_ref_1.fetch_add(
                    state.inner().gas_used_value(),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }),
        )]),
    ]);

    gas_consumed_attester_ref_2.load(std::sync::atomic::Ordering::SeqCst)
}

#[test]
fn try_unbond_successful() {
    let (mut runner, attester, _, _) = setup();

    let attester_address = attester.user_info.address();
    let attester_bond = attester.bond;
    let attester_balance = attester.user_info.balance();

    let gas_consumed_start_unbonding =
        check_attester_bonded_and_start_unbond(&mut runner, &attester);

    let gas_consumed_attester_ref_1 = Arc::new(AtomicU64::new(gas_consumed_start_unbonding));
    let gas_consumed_attester_ref_2 = gas_consumed_attester_ref_1.clone();

    runner.execute_slots(vec![
        // Execute empty slots to finalize unbonding. Artificially increase the light client finalized height.
        SlotTestCase::empty().with_end_slot_hook(Box::new(move |state| {
            // Increase the light client finalized height
            TestAttesterIncentives::default()
                .light_client_finalized_height
                .set(&(INIT_BONDING_HEIGHT + TEST_ROLLUP_FINALITY_PERIOD), state)
                .unwrap_infallible();
        })),
        // Finalize unbonding
        SlotTestCase::from_rewarded_batch(vec![TxTestCase::<RT, _, _>::applied_with_hook(
            attester.create_plain_message::<TestAttesterIncentives>(
                sov_attester_incentives::CallMessage::EndUnbondingAttester,
            ),
            Box::new(move |state| {
                // Test that the unbonding attester event is emitted
                assert!(state.inner().events().iter().any(|event| {
                    event.downcast_ref::<Event<S>>()
                        == Some(&Event::UnbondedAttester {
                            amount_withdrawn: attester_bond,
                        })
                }));

                assert_eq!(
                    TestAttesterIncentives::default()
                        .unbonding_attesters
                        .get(&attester_address, state)
                        .unwrap_infallible(),
                    None,
                    "The attester should not be part of the unbonding set anymore"
                );

                assert_eq!(
                    TestAttesterIncentives::default()
                        .bonded_attesters
                        .get(&attester_address, state)
                        .unwrap_infallible(),
                    None,
                    "The attester should not be part of the bonded set anymore"
                );

                gas_consumed_attester_ref_1.fetch_add(
                    state.inner().gas_used_value(),
                    std::sync::atomic::Ordering::SeqCst,
                );
            }),
        )])
        .with_end_slot_hook(Box::new(move |state| {
            // Check the final balance of the attester
            assert_eq!(
                TestRunner::<RT, S>::bank_gas_balance(&attester_address, state),
                Some(
                    attester_balance + attester_bond
                        - gas_consumed_attester_ref_2.load(std::sync::atomic::Ordering::SeqCst)
                )
            );
        })),
    ]);
}

#[test]
fn try_unbond_too_early() {
    let (mut runner, attester, _, _) = setup();

    check_attester_bonded_and_start_unbond(&mut runner, &attester);

    // Finalize unbonding, this should fail because the unbonding period has not passed yet
    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::reverted(
            attester.create_plain_message::<TestAttesterIncentives>(
                sov_attester_incentives::CallMessage::EndUnbondingAttester,
            ),
            ModuleError(
                AttesterIncentiveErrors::<StateAccessorError<<S as Spec>::Gas>>::UnbondingNotFinalized.into(),
            ),
        ),
    ])]);
}

/// The attester tries to unbond without bonding
#[test]
fn try_unbond_without_bonding() {
    let (mut runner, _, additional_account, _) = setup();

    let additional_account_address = additional_account.user_info.address();

    runner.execute_slots(vec![
        SlotTestCase::empty().with_end_slot_hook(Box::new(move |state| {
            // Check that the additional account is not bonded
            assert_eq!(
                TestAttesterIncentives::default()
                    .bonded_attesters
                    .get(&additional_account_address, state)
                    .unwrap(),
                None,
                "The additional account should not be bonded"
            );
        })),
        SlotTestCase::from_rewarded_batch(vec![TxTestCase::<RT, _, _>::applied_with_hook(
            additional_account.create_plain_message::<TestAttesterIncentives>(
                sov_attester_incentives::CallMessage::BeginUnbondingAttester,
            ),
            Box::new(move |state| {
                assert_eq!(
                    TestAttesterIncentives::default()
                        .unbonding_attesters
                        .get(&additional_account_address, state)
                        .unwrap(),
                    None,
                    "The additional account should not be part of the unbonding set"
                );
            }),
        )]),
    ]);
}

/// The attester tries to unbond without waiting for the two-phase unbonding to finalize
#[test]
fn try_skip_two_phase_unbonding() {
    let (mut runner, attester, _, _) = setup();

    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::reverted(
            attester.create_plain_message::<TestAttesterIncentives>(
                sov_attester_incentives::CallMessage::EndUnbondingAttester,
            ),
            ModuleError(
                AttesterIncentiveErrors::<StateAccessorError<<S as Spec>::Gas>>::AttesterIsNotUnbonding.into(),
            ),
        ),
    ])]);
}

/// The attester tries to bond while unbonding
#[test]
fn try_bond_while_unbonding() {
    let (mut runner, attester, _, _) = setup();
    let attester_address = attester.user_info.address();
    let attester_bond = attester.bond;

    runner.execute_slots(vec![
        // The attester starts unbonding
        SlotTestCase::from_rewarded_batch(vec![TxTestCase::<RT, _, _>::applied_with_hook(
            attester.create_plain_message::<TestAttesterIncentives>(
                sov_attester_incentives::CallMessage::BeginUnbondingAttester,
            ),
            Box::new(move |state| {
                // Check that the state has been updated correctly
                assert_eq!(
                    TestAttesterIncentives::default()
                        .unbonding_attesters
                        .get(&attester_address, state)
                        .unwrap(),
                    Some(UnbondingInfo {
                        unbonding_initiated_height: 0,
                        amount: attester_bond
                    }),
                    "The attester should be part of the unbonding set"
                );

                assert_eq!(
                    TestAttesterIncentives::default()
                        .bonded_attesters
                        .get(&attester_address, state)
                        .unwrap(),
                    None,
                    "The attester should not be bonded"
                );
            }),
        )]),
        // The attester shouldn't be able to bond while unbonding
        SlotTestCase::from_rewarded_batch(vec![TxTestCase::reverted(
            attester.create_plain_message(sov_attester_incentives::CallMessage::BondAttester(100)),
            ModuleError(
                AttesterIncentiveErrors::<StateAccessorError<<S as Spec>::Gas>>::AttesterIsUnbonding.into(),
            ),
        )]),
    ]);
}
