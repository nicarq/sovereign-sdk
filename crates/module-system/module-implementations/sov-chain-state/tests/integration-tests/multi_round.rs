use sov_chain_state::ChainState;
use sov_modules_api::da::Time;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{ApiStateAccessor, Gas, GasArray, Spec, VersionReader};
use sov_test_utils::runtime::TestApplySlotOutput;
use sov_test_utils::{get_gas_used, AsUser, BatchType, TestUser};
use sov_value_setter::ValueSetter;

use crate::{setup, TestChainStateRuntime, RT, S};

const NUM_ROUNDS: u64 = 4;
const NUM_TXS_PER_ROUND: usize = 10;

fn generate_admin_messages(
    admin: &TestUser<S>,
    round_num: usize,
    num_messages: usize,
) -> BatchType<RT, S> {
    let mut messages = Vec::with_capacity(num_messages);

    for i in 0..num_messages {
        messages.push(admin.create_plain_message::<RT, ValueSetter<S>>(
            sov_value_setter::CallMessage::SetValue((round_num + i) as u32),
        ));
    }

    BatchType(messages)
}

fn check_chain_state_update(
    num_rounds: u64,
    txs_to_send_per_round: usize,
    post_round_closure: &mut impl FnMut(
        // Round number
        u64,
        // Kernel working set
        &mut ApiStateAccessor<S>,
        // The immediate result of the apply slot function
        TestApplySlotOutput<TestChainStateRuntime<S>, S>,
    ),
) {
    let (admin, mut runner) = setup();

    for round in 0..num_rounds {
        let result = runner.execute(generate_admin_messages(
            &admin,
            round as usize,
            txs_to_send_per_round,
        ));

        // Sanity check: there should be only one batch executed
        assert_eq!(result.batch_receipts.len(), 1);

        runner.query_state(|state| post_round_closure(round, state, result));
    }
}

#[test]
fn test_chain_state_update_gas_used() {
    check_chain_state_update(
        NUM_ROUNDS,
        NUM_TXS_PER_ROUND,
        &mut |_round, kernel, result| {
            let expected_gas_consumed_batch = result.batch_receipts[0].tx_receipts.iter().fold(
                <<S as Spec>::Gas as Gas>::zero(),
                |mut acc, tx_receipt| {
                    acc.combine(&get_gas_used(tx_receipt));
                    acc
                },
            );

            let in_progress_tx = ChainState::<S>::default()
                .last_slot(kernel)
                .unwrap_infallible()
                .unwrap();

            assert_eq!(
                in_progress_tx.gas_used(),
                &expected_gas_consumed_batch,
                "The gas used should be the sum of the gas used by the transactions in the batch"
            );
        },
    );
}

fn nanosecs(time: Time) -> u64 {
    let time_subsec_u64 = time.subsec_nanos() as u64;
    (time.secs() * 1_000_000_000) as u64 + time_subsec_u64
}

#[test]
fn test_chain_state_update_time() {
    let mut previous_time = 0;

    check_chain_state_update(
        NUM_ROUNDS,
        NUM_TXS_PER_ROUND,
        &mut |_round, kernel, _result| {
            let current_time = nanosecs(ChainState::<S>::default().get_time(kernel).unwrap());

            assert!(
                previous_time < current_time,
                "The time should be lower than the current time"
            );

            previous_time = current_time;
        },
    );
}

#[test]
fn test_chain_state_update_state_root() {
    let mut previous_state_root = None;

    check_chain_state_update(
        NUM_ROUNDS,
        NUM_TXS_PER_ROUND,
        &mut |round, kernel, result| {
            if round == 0 {
                previous_state_root = Some(result.state_root);
            } else {
                let previous_transition = ChainState::<S>::default()
                    .get_historical_transitions(round, kernel)
                    .unwrap_infallible()
                    .unwrap();

                assert_eq!(
                    previous_transition.post_state_root(),
                    &previous_state_root.unwrap(),
                    "The state roots don't match"
                );

                previous_state_root = Some(result.state_root);
            }
        },
    );
}

#[test]
fn test_chain_state_kernel_updates() {
    check_chain_state_update(
        NUM_ROUNDS,
        NUM_TXS_PER_ROUND,
        &mut |round, state, _result| {
            assert_eq!(
                state.rollup_height_to_access(),
                round + 1,
                "The kernel should be updated to the current round"
            );
        },
    );
}

#[test]
fn test_chain_state_update_transitions() {
    let mut historical_transitions = Vec::new();

    check_chain_state_update(
        NUM_ROUNDS,
        NUM_TXS_PER_ROUND,
        &mut |round, kernel, _result| {
            if round == 0 {
                let in_progress_transition = ChainState::<S>::default()
                    .last_slot(kernel)
                    .unwrap_infallible()
                    .unwrap();
                historical_transitions.push(in_progress_transition);
            } else {
                for (i, historical_transition) in historical_transitions.iter().enumerate() {
                    let expected_previous_transition = historical_transition;

                    let stored_previous_transition = ChainState::<S>::default()
                        .get_historical_transitions((i + 1) as u64, kernel)
                        .unwrap_infallible()
                        .unwrap();

                    assert_eq!(
                        expected_previous_transition.gas_limit(),
                        stored_previous_transition.gas_limit(),
                        "The gas limits don't match"
                    );

                    assert_eq!(
                        expected_previous_transition.gas_price(),
                        stored_previous_transition.gas_price(),
                        "The gas prices don't match"
                    );

                    assert_eq!(
                        expected_previous_transition.gas_used(),
                        stored_previous_transition.gas_used(),
                        "The gas used doesn't match"
                    );

                    assert_eq!(
                        expected_previous_transition.hash(),
                        stored_previous_transition.slot_hash(),
                        "The slot hashes don't match"
                    );
                }

                historical_transitions.push(
                    ChainState::<S>::default()
                        .last_slot(kernel)
                        .unwrap_infallible()
                        .unwrap(),
                );
            }
        },
    );
}
