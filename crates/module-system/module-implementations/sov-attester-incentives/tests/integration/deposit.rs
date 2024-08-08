use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use sov_bank::{Bank, GAS_TOKEN_ID};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::GasMeter;
use sov_test_utils::{AsUser, SlotTestCase, TxTestCase};

use crate::helpers::{setup, TestAttesterIncentives, RT, S};

#[test]
fn deposit_successful() {
    let (mut runner, attester, _, _) = setup();

    let attester_address = attester.user_info.address();
    let starting_free_balance = attester.user_info.balance();
    let starting_bond = attester.bond;
    let extra_bond = 0;
    let gas_cost = Arc::new(AtomicU64::new(0));
    let gas_cost_ref1 = gas_cost.clone();

    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<RT, _, _>::applied_with_hook(
            attester.create_plain_message::<TestAttesterIncentives>(
                sov_attester_incentives::CallMessage::DepositAttester(extra_bond),
            ),
            Box::new(move |state| {
                {
                    gas_cost.fetch_add(
                        state.inner().gas_used_value(),
                        std::sync::atomic::Ordering::SeqCst,
                    );
                }

                assert_eq!(
                    TestAttesterIncentives::default()
                        .bonded_attesters
                        .get(&attester_address, state)
                        .unwrap(),
                    Some(starting_bond + extra_bond),
                );
            }),
        ),
    ])
    .with_end_slot_hook(Box::new(move |state| {
        let total_gas_cost = gas_cost_ref1.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            Bank::<S>::default()
                .get_balance_of(&attester_address, GAS_TOKEN_ID, state)
                .unwrap_infallible(),
            Some(starting_free_balance - extra_bond - total_gas_cost),
        );
    }))]);
}
