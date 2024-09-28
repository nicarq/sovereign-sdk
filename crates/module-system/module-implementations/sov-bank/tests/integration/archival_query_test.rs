use sov_bank::{Bank, Coins};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_test_utils::{AsUser, TestSpec};

use crate::helpers::*;

/// Tests the archival query functionality of the bank module by doing some transfers and querying the old balances afterwards.
#[test]
fn transfer_token_and_query_old_balances() {
    let (
        TestData {
            token_name,
            token_id,
            user_high_token_balance: sender,
            user_no_token_balance: receiver,
            ..
        },
        mut runner,
    ) = setup();

    let sender_initial_token_balance = sender.token_balance(&token_name).unwrap();

    const AMOUNT_PER_TRANSFER: u64 = 10;

    for i in 1..10 {
        runner.execute(sender.create_plain_message::<sov_bank::Bank<TestSpec>>(
            sov_bank::CallMessage::Transfer {
                to: receiver.address(),
                coins: Coins {
                    amount: AMOUNT_PER_TRANSFER,
                    token_id,
                },
            },
        ));

        // Test queries for latest height.
        runner.query_state(|state| {
            assert_eq!(
                Bank::<TestSpec>::default()
                    .get_balance_of(&receiver.address(), token_id, state)
                    .unwrap_infallible(),
                Some(AMOUNT_PER_TRANSFER * i)
            );
        });

        for j in 1..i {
            runner.query_state(|state| {
                let archival_state = &mut state.get_archival_at(j);
                assert_eq!(
                    Bank::<TestSpec>::default()
                        .get_balance_of(&sender.address(), token_id, archival_state)
                        .unwrap_infallible(),
                    Some(sender_initial_token_balance - AMOUNT_PER_TRANSFER * (j - 1))
                );

                if j > 1 {
                    assert_eq!(
                        Bank::<TestSpec>::default()
                            .get_balance_of(&receiver.address(), token_id, archival_state)
                            .unwrap_infallible(),
                        Some(AMOUNT_PER_TRANSFER * (j - 1))
                    );
                } else {
                    assert_eq!(
                        Bank::<TestSpec>::default()
                            .get_balance_of(&receiver.address(), token_id, archival_state)
                            .unwrap_infallible(),
                        None
                    );
                }
            });
        }
    }
}
