use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use sov_bank::{Bank, GAS_TOKEN_ID};
use sov_mock_da::MockDaSpec;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::GasMeter;
use sov_prover_incentives::{CallMessage, Event};
use sov_test_utils::{AsUser, SlotTestCase, TransactionTestCase, TxTestCase};

use crate::helpers::{
    __GeneratedRuntimeInternalsEvent, setup, ProverRuntime, TestProverIncentives, RT,
};

pub(crate) type S = sov_test_utils::TestSpec;

#[test]
fn test_genesis_bond() {
    let (mut runner, genesis_prover, _) = setup();

    runner.query_state(|state| {
        assert_eq!(
            TestProverIncentives::default()
                .bonded_provers
                .get(&genesis_prover.user_info.address(), state)
                .unwrap(),
            Some(genesis_prover.bond),
            "The genesis prover should be bonded"
        );
        assert_eq!(
            Bank::<S>::default()
                .get_balance_of(&genesis_prover.user_info.address(), GAS_TOKEN_ID, state)
                .unwrap_infallible(),
            Some(genesis_prover.user_info.available_balance),
            "The balance of the prover should be equal to the free balance"
        );
    });
}

#[test]
fn test_topup_existing_bond() {
    let (mut runner, genesis_prover, _) = setup();

    let starting_free_balance = genesis_prover.user_info.balance();
    let starting_bond = genesis_prover.bond;
    let extra_bond_amount = 50;
    let prover_address = genesis_prover.user_info.address();

    let test = TransactionTestCase::<S, RT, TestProverIncentives> {
        assert: Box::new(move |result, state| {
            assert!(result.outcome.is_successful());
            assert!(result.events.iter().any(|event| matches!(
                event,
                __GeneratedRuntimeInternalsEvent::prover_incentives(Event::Deposited {
                    prover,
                    deposit
                }) if *prover == prover_address && *deposit == extra_bond_amount
            )));
            assert_eq!(
                TestProverIncentives::default()
                    .bonded_provers
                    .get(&prover_address, state)
                    .unwrap(),
                Some(starting_bond + extra_bond_amount),
            );
            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&prover_address, GAS_TOKEN_ID, state)
                    .unwrap_infallible(),
                Some(starting_free_balance - extra_bond_amount - result.gas_used),
            );
        }),
        input: genesis_prover
            .create_plain_message::<TestProverIncentives>(CallMessage::Deposit(extra_bond_amount)),
    };

    runner.execute_transaction(test);
}

// Note: we are bonding less than `minimum_bond` amount, currently this is allowed
// as users are able to deposit more bond and we check the user is sufficiently
// bonded when processing submitted proofs.
#[test]
fn test_bonding_new_prover() {
    let (mut runner, _, unbonded_user) = setup();

    let starting_free_balance = unbonded_user.balance();
    let bond_amount = 100000001;
    let user_address = unbonded_user.address();
    let gas_cost = Arc::new(AtomicU64::new(0));
    let gas_cost_ref1 = gas_cost.clone();

    runner.execute_slots::<TestProverIncentives>(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<ProverRuntime<S, MockDaSpec>, _, _>::applied_with_hook(
            unbonded_user
                .create_plain_message::<TestProverIncentives>(CallMessage::Register(bond_amount)),
            Box::new(move |ws| {
                {
                    gas_cost.fetch_add(
                        ws.inner().gas_used_value(),
                        std::sync::atomic::Ordering::SeqCst,
                    );
                }
                assert!(
                    ws.inner().events().iter().any(|event| matches!(
                        event.downcast_ref::<Event<S>>(),
                        Some(Event::Registered {
                            prover,
                            amount,

                        }) if *prover == user_address
                            && *amount == bond_amount
                    )),
                    "Event with expected bonding values not found"
                );
            }),
        ),
    ])
    .with_end_slot_hook(Box::new(move |state| {
        assert_eq!(
            TestProverIncentives::default()
                .bonded_provers
                .get(&unbonded_user.address(), state)
                .unwrap(),
            Some(bond_amount),
        );

        let total_gas_cost = gas_cost_ref1.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            Bank::<S>::default()
                .get_balance_of(&unbonded_user.address(), GAS_TOKEN_ID, state)
                .unwrap_infallible(),
            Some(starting_free_balance - bond_amount - total_gas_cost),
        );
    }))]);
}

#[test]
fn test_unbonding() {
    let (mut runner, genesis_prover, _) = setup();

    let expected_final_balance =
        Arc::new(AtomicU64::new(genesis_prover.user_info.available_balance));
    let expected_balance_ref1 = expected_final_balance.clone();
    let genesis_prover_address = genesis_prover.user_info.address();
    let genesis_prover_bond = genesis_prover.bond;

    runner.execute_slots::<TestProverIncentives>(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<ProverRuntime<S, MockDaSpec>, _, _>::applied_with_hook(
            genesis_prover.create_plain_message::<TestProverIncentives>(CallMessage::Exit),
            Box::new(move |ws| {
                {
                    // Pay for gas from the provers balance
                    expected_final_balance.fetch_sub(
                        ws.inner().gas_used_value(),
                        std::sync::atomic::Ordering::SeqCst,
                    );

                    expected_final_balance
                        .fetch_add(genesis_prover_bond, std::sync::atomic::Ordering::SeqCst);
                }
                assert!(ws.inner().events().iter().any(|event| matches!(
                    event.downcast_ref::<Event<S>>(),
                    Some(Event::Exited { .. })
                )));
            }),
        ),
    ])
    .with_end_slot_hook(Box::new(move |state| {
        assert_eq!(
            expected_balance_ref1.load(std::sync::atomic::Ordering::SeqCst),
            Bank::<S>::default()
                .get_balance_of(&genesis_prover_address, GAS_TOKEN_ID, state)
                .unwrap_infallible()
                .unwrap()
        );
    }))]);
}
