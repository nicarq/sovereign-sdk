use sov_bank::ReserveGasErrorReason;
use sov_modules_api::capabilities::FatalError;
use sov_modules_api::macros::config_value;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_stf_blueprint::SkippedReason;
use sov_value_setter::{ValueSetter, ValueSetterConfig};

use crate::interface::AsUser;
use crate::runtime::optimistic::HighLevelOptimisticGenesisConfig;
use crate::runtime::TestRunner;
use crate::{generate_optimistic_runtime, MockDaSpec, SlotTestCase, TestUser, TxTestCase};

type S = crate::TestSpec;

generate_optimistic_runtime!(TestRuntime <= value_setter: ValueSetter<S>);

/// Sets up a test runner with the [`ValueSetter`] with a single additional admin account.
fn setup() -> (TestUser<S>, TestRunner<TestRuntime<S, MockDaSpec>, S>) {
    let genesis_config = HighLevelOptimisticGenesisConfig::generate_with_additional_accounts(1);

    let admin = genesis_config.additional_accounts.first().unwrap().clone();

    let value_setter_config = ValueSetterConfig {
        admin: admin.address(),
    };

    // Run genesis registering the attester and sequencer we've generated.
    let genesis = GenesisConfig::from_minimal_config(genesis_config.into(), value_setter_config);

    let runner =
        TestRunner::new_with_genesis(genesis.into_genesis_params(), TestRuntime::default());

    (admin, runner)
}

#[test]
fn test_query_runtime() {
    let (admin, mut runner) = setup();

    let admin_genesis_address = runner.query_state(|state| {
        assert_eq!(
            ValueSetter::<S>::default()
                .value
                .get(state)
                .unwrap_infallible(),
            None,
            "The value should not be set"
        );

        ValueSetter::<S>::default()
            .admin
            .get(state)
            .unwrap_infallible()
            .expect("The admin should be set")
    });

    assert_eq!(
        admin.address(),
        admin_genesis_address,
        "The admins don't match"
    );

    runner.execute_slots(vec![SlotTestCase::from_rewarded_batch(vec![TxTestCase::<
        TestRuntime<S, MockDaSpec>,
        _,
        _,
    >::applied(
        admin.create_plain_message::<ValueSetter<S>>(sov_value_setter::CallMessage::SetValue(1)),
    )])]);

    let state_value = runner.query_state(|state| {
        ValueSetter::<S>::default()
            .value
            .get(state)
            .unwrap_infallible()
    });

    assert_eq!(state_value, Some(1), "The value should be set to 1");
}

/// Checks that the chain id of a transaction can be overridden.
#[test]
fn test_custom_transaction_details_chain_id() {
    let (admin, mut runner) = setup();

    let real_chain_id = config_value!("CHAIN_ID");
    let fake_chain_id = real_chain_id + 1;

    runner.execute_slots::<ValueSetter<S>>(vec![SlotTestCase::from_slashed_batch(
        vec![TxTestCase::<TestRuntime<S, MockDaSpec>, _, _>::dropped(
            admin
                .create_plain_message::<ValueSetter<S>>(sov_value_setter::CallMessage::SetValue(1))
                .with_chain_id(fake_chain_id),
        )],
        FatalError::InvalidChainId {
            expected: real_chain_id,
            got: fake_chain_id,
        },
    )]);
}

/// Checks that the chain id of a transaction can be overridden.
#[test]
fn test_custom_transaction_details_max_fee() {
    let (admin, mut runner) = setup();

    runner.execute_slots::<ValueSetter<S>>(vec![SlotTestCase::from_rewarded_batch(vec![
        TxTestCase::<TestRuntime<S, MockDaSpec>, _, _>::skipped(
            admin
                .create_plain_message::<ValueSetter<S>>(sov_value_setter::CallMessage::SetValue(10))
                .with_max_fee(
                     0,
                ),
            SkippedReason::CannotReserveGas(
                // TODO(@theochap): make it possible to inject closures to test the error message
                ReserveGasErrorReason::<S>::InsufficientGasForPreExecutionChecks("The gas to charge is greater than the funds available in the meter. Gas to charge GasUnit[2261, 2261], gas price GasPrice[10, 10], remaining funds 0, total gas consumed GasUnit[0, 0]".to_string())
                    .to_string(),
            ),
        ),
    ])]);
}
