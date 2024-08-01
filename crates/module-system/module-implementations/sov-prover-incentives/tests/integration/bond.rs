use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use sov_bank::{Bank, GAS_TOKEN_ID};
use sov_mock_da::MockDaSpec;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::GasMeter;
use sov_prover_incentives::{CallMessage, Event};
use sov_test_utils::{MessageType, SlotTestCase, TxTestCase};

use crate::helpers::{setup, ProverRuntime, TestProverIncentives};

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
fn test_unbonding() {
    let (mut runner, genesis_prover, _) = setup();

    let expected_final_balance =
        Arc::new(AtomicU64::new(genesis_prover.user_info.available_balance));
    let expected_balance_ref1 = expected_final_balance.clone();
    let genesis_prover_address = genesis_prover.user_info.address();
    let genesis_prover_bond = genesis_prover.bond;
    let genesis_prover_key = genesis_prover.user_info.private_key();

    runner.execute_slots::<TestProverIncentives>(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<ProverRuntime<S, MockDaSpec>, _, _>::applied_with_hook(
            MessageType::Plain(CallMessage::UnbondProver, genesis_prover_key.clone()),
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
                    Some(Event::UnBondedProver { .. })
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
