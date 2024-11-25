use sov_bank::{config_gas_token_id, Bank};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_test_utils::{AsUser, TransactionTestCase};

use crate::helpers::{setup, TestAttesterIncentives, RT, S};

#[test]
fn test_deposit_successful() {
    let (mut runner, attester, _, _) = setup();

    let attester_address = attester.user_info.address();
    let starting_free_balance = attester.user_info.balance();
    let starting_bond = attester.bond;
    let extra_bond = 0;

    runner.execute_transaction(TransactionTestCase {
        input: attester.create_plain_message::<RT, TestAttesterIncentives>(
            sov_attester_incentives::CallMessage::DepositAttester(extra_bond),
        ),
        assert: Box::new(move |result, state| {
            assert_eq!(
                TestAttesterIncentives::default()
                    .bonded_attesters
                    .get(&attester_address, state)
                    .unwrap(),
                Some(starting_bond + extra_bond),
            );
            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&attester_address, config_gas_token_id(), state)
                    .unwrap_infallible(),
                Some(starting_free_balance - extra_bond - result.gas_value_used),
            );
        }),
    });
}
