use sov_modules_api::prelude::UnwrapInfallible;
use sov_value_setter::{ValueSetter, ValueSetterConfig};

use crate::interface::AsUser;
use crate::runtime::optimistic::HighLevelOptimisticGenesisConfig;
use crate::runtime::TestRunner;
use crate::{generate_optimistic_runtime, MockDaSpec, SlotTestCase, TxTestCase};

type S = crate::TestSpec;

#[test]
fn test_query_runtime() {
    generate_optimistic_runtime!(TestRuntime <= value_setter: ValueSetter<S>);

    let genesis_config = HighLevelOptimisticGenesisConfig::generate_with_additional_accounts(1);

    let admin = genesis_config.additional_accounts.first().unwrap().clone();

    let value_setter_config = ValueSetterConfig {
        admin: admin.address(),
    };

    // Run genesis registering the attester and sequencer we've generated.
    let genesis = GenesisConfig::from_minimal_config(genesis_config.into(), value_setter_config);

    let mut runner =
        TestRunner::new_with_genesis(genesis.into_genesis_params(), TestRuntime::default());

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
