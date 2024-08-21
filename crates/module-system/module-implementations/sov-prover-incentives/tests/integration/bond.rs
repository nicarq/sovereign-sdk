use sov_bank::{Bank, GAS_TOKEN_ID};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_prover_incentives::{CallMessage, Event};
use sov_test_utils::{AsUser, TransactionTestCase};

use crate::helpers::{setup, TestProverIncentives, TestRuntimeEvent, RT};

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
            Some(genesis_prover.user_info.available_gas_balance),
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
        input: genesis_prover
            .create_plain_message::<TestProverIncentives>(CallMessage::Deposit(extra_bond_amount)),
        assert: Box::new(move |result, state| {
            assert!(result.events.iter().any(|event| matches!(
                event,
                TestRuntimeEvent::prover_incentives(Event::Deposited {
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

    runner.execute_transaction(TransactionTestCase {
        input: unbonded_user
            .create_plain_message::<TestProverIncentives>(CallMessage::Register(bond_amount)),
        assert: Box::new(move |result, state| {
            assert!(result.events.iter().any(|event| matches!(
                event,
                TestRuntimeEvent::prover_incentives(Event::Registered {
                    prover,
                    amount
                }) if *prover == user_address && *amount == bond_amount
            )));
            assert_eq!(
                TestProverIncentives::default()
                    .bonded_provers
                    .get(&unbonded_user.address(), state)
                    .unwrap(),
                Some(bond_amount),
            );
            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&unbonded_user.address(), GAS_TOKEN_ID, state)
                    .unwrap_infallible(),
                Some(starting_free_balance - bond_amount - result.gas_used),
            );
        }),
    });
}

#[test]
fn test_unbonding() {
    let (mut runner, genesis_prover, _) = setup();

    runner.execute_transaction(TransactionTestCase {
        input: genesis_prover.create_plain_message::<TestProverIncentives>(CallMessage::Exit),
        assert: Box::new(move |result, state| {
            assert!(result.events.iter().any(|event| matches!(
                event,
                TestRuntimeEvent::prover_incentives(Event::Exited { .. })
            )));
            assert_eq!(
                genesis_prover.user_info.available_gas_balance + genesis_prover.bond
                    - result.gas_used,
                Bank::<S>::default()
                    .get_balance_of(&genesis_prover.user_info.address(), GAS_TOKEN_ID, state)
                    .unwrap_infallible()
                    .unwrap()
            );
        }),
    });
}
