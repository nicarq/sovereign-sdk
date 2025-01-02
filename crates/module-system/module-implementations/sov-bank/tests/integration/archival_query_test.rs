use sov_bank::{Bank, Coins};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_rollup_interface::common::IntoSlotNumber;
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

    for height in 1..10 {
        runner.execute(sender.create_plain_message::<RT, sov_bank::Bank<TestSpec>>(
            sov_bank::CallMessage::Transfer {
                to: receiver.address(),
                coins: Coins {
                    amount: AMOUNT_PER_TRANSFER,
                    token_id,
                },
            },
        ));

        // Test queries for latest height.
        runner.query_visible_state(|state| {
            assert_eq!(
                Bank::<TestSpec>::default()
                    .get_balance_of(&receiver.address(), token_id, state)
                    .unwrap_infallible(),
                Some(AMOUNT_PER_TRANSFER * height)
            );
        });

        for height_to_query in 0..=height {
            runner.query_visible_state(|state| {
                let archival_state = &mut state
                    .state_at_height(height_to_query.to_visible_slot_number())
                    .unwrap();

                // Sender query deducted at every height
                assert_eq!(
                    Bank::<TestSpec>::default()
                        .get_balance_of(&sender.address(), token_id, archival_state)
                        .unwrap_infallible(),
                    Some(sender_initial_token_balance - AMOUNT_PER_TRANSFER * height_to_query)
                );

                let expected_receiver_balance = if height_to_query == 0 {
                    None
                } else {
                    Some(AMOUNT_PER_TRANSFER * height_to_query)
                };
                assert_eq!(
                    Bank::<TestSpec>::default()
                        .get_balance_of(&receiver.address(), token_id, archival_state)
                        .unwrap_infallible(),
                    expected_receiver_balance
                );
            });
        }
    }
}
