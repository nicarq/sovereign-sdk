use std::collections::HashMap;

use sov_attester_incentives::{AttesterIncentives, CallMessage};
use sov_bank::{config_gas_token_id, Bank};
use sov_modules_api::macros::config_value;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{ApiStateAccessor, Gas, GasArray, GasMeter, Spec, TxEffect};
use sov_test_utils::runtime::TestRunner;
use sov_test_utils::{AsUser, BatchTestCase, BatchType, TestUser, TransactionType};

use crate::helpers::{
    minimal_attester_bond, minimal_challenger_bond, setup_with_custom_runtime, RT, S,
};

enum TestRole {
    Attester,
    Challenger,
}

impl TestRole {
    fn minimal_bond(&self, runner: &TestRunner<RT, S>) -> u64 {
        match self {
            TestRole::Attester => minimal_attester_bond(runner),
            TestRole::Challenger => minimal_challenger_bond(runner),
        }
    }

    fn user_bond(&self, user: &TestUser<S>, state: &mut ApiStateAccessor<S>) -> Option<u64> {
        match self {
            TestRole::Attester => AttesterIncentives::<S>::default()
                .bonded_attesters
                .get(&user.address(), state)
                .unwrap_infallible(),
            TestRole::Challenger => AttesterIncentives::<S>::default()
                .bonded_challengers
                .get(&user.address(), state)
                .unwrap_infallible(),
        }
    }

    fn create_call_message(&self, bond_amount: u64) -> CallMessage {
        match self {
            TestRole::Attester => CallMessage::RegisterAttester(bond_amount),
            TestRole::Challenger => CallMessage::RegisterChallenger(bond_amount),
        }
    }
}

/// This test ensures that the attester cannot send proofs when the gas price is too high.
/// That is, if the gas price increases, the attester will not have bonded enough funds and then won't be able to prove anymore.
/// Currently, the easiest way to do this is to artificially change the gas cost of some operation in the bank module. We do that
/// by modifying the runtime manually.
fn test_cannot_prove_when_gas_price_is_too_high(role: TestRole) {
    let mut gas_limit = <<S as Spec>::Gas>::from(config_value!("INITIAL_GAS_LIMIT"));
    let gas_target = gas_limit.scalar_division(2).clone();

    let zero_gas = <S as Spec>::Gas::zero();

    let mut runtime = RT::default();

    runtime
        .bank
        .override_gas_config(sov_bank::BankGasConfig::<<S as Spec>::Gas> {
            burn: gas_target.clone(),
            mint: zero_gas.clone(),
            create_token: zero_gas.clone(),
            transfer: zero_gas.clone(),
            freeze: zero_gas.clone(),
        });

    let (mut runner, _, _, user) = setup_with_custom_runtime(runtime);

    let mut nonces = HashMap::new();

    let additional_user_bond = role.minimal_bond(&runner);

    let initial_gas_price = runner.query_visible_state(|state| state.gas_info().gas_price);

    let bank_signed = user
        .create_plain_message::<RT, Bank<S>>(sov_bank::CallMessage::Burn {
            coins: sov_bank::Coins {
                amount: 1,
                token_id: config_gas_token_id(),
            },
        })
        .with_max_fee(user.available_gas_balance / 2)
        .to_serialized_authenticated_tx(&mut nonces);

    let register_signed = user
        .create_plain_message::<RT, AttesterIncentives<S>>({
            role.create_call_message(additional_user_bond)
        })
        .to_serialized_authenticated_tx(&mut nonces);

    // We execute a batch of two transactions, check that the total gas used is higher than the target.
    runner.execute_batch(BatchTestCase {
        input: BatchType(vec![
            TransactionType::<RT, S>::PreAuthenticated(bank_signed),
            TransactionType::<RT, S>::PreAuthenticated(register_signed),
        ]),
        assert: Box::new(move |result, _state| {
            assert_eq!(result.batch_receipt.clone().unwrap().tx_receipts.len(), 2);

            let mut total_gas_used = <S as Spec>::Gas::zero();

            for (i, tx_receipt) in result.batch_receipt.unwrap().tx_receipts.iter().enumerate() {
                match &tx_receipt.receipt {
                    TxEffect::Successful(tx_contents) => {
                        total_gas_used.combine(&tx_contents.gas_used);
                    }
                    _ => {
                        panic!("Tx {i} with receipt {tx_receipt:?} should be successful");
                    }
                }
            }

            assert!(
                total_gas_used > gas_target,
                "The total gas used should be higher than the initial gas used"
            );
        }),
    });

    // We need to advance by one slot to update the gas price
    runner.advance_slots(1);

    let new_bond_amount = role.minimal_bond(&runner);

    runner.query_visible_state(|state| {
        let new_gas_price = state.gas_info().gas_price;

        assert!(
            new_gas_price > initial_gas_price,
            "The new gas price {new_gas_price} should be higher than the initial gas price {initial_gas_price}"
        );

        assert!(
            new_bond_amount > additional_user_bond,
            "The new bond amount {new_bond_amount} should be higher than the initial additional user bond {additional_user_bond}."
        );

        let user = role.user_bond(&user, state);

        // The user should be registered
        assert!(
           user.
           is_some(),
            "The additional user should be registered"
        );

        // But he should not be allowed to send transactions because he doesn't have enough stake.
        assert!(
            user.unwrap() < new_bond_amount,
            "The user should not be allowed to send transactions because he doesn't have enough stake."
        );
    });
}

#[test]
fn test_cannot_attest_when_gas_price_is_too_high() {
    test_cannot_prove_when_gas_price_is_too_high(TestRole::Attester);
}

#[test]
fn test_cannot_challenge_when_gas_price_is_too_high() {
    test_cannot_prove_when_gas_price_is_too_high(TestRole::Challenger);
}
