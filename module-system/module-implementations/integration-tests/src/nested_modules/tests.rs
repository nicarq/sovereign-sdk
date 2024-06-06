use sov_modules_api::{ModulePrefix, Spec, StateMap, WorkingSet};
use sov_prover_storage_manager::new_orphan_storage;
use sov_state::{Storage, ZkStorage};
use sov_test_utils::{TestSpec, ZkTestSpec};

use super::helpers::{module_c, Event};

#[test]
fn nested_module_call_test() {
    let tmpdir = tempfile::tempdir().unwrap();
    let prover_storage = new_orphan_storage(tmpdir.path()).unwrap();
    let mut state = WorkingSet::new(prover_storage.clone());

    // Test the `native` execution.
    {
        execute_module_logic::<TestSpec>(&mut state);
        test_state_update::<TestSpec>(&mut state);
    }

    let events: Vec<Event> = state
        .take_events() // This should take all events at once
        .into_iter() // Consume the Vec<TypedEvent>
        .map(|typed_event| typed_event.downcast::<Event>().unwrap()) // Downcast each TypedEvent
        .collect();

    assert_eq!(
        events,
        vec![
            Event::Execute,
            Event::Update,
            Event::Update,
            Event::Update,
            Event::Update,
        ]
    );

    let (log, _, witness) = state.checkpoint().0.freeze();
    prover_storage
        .validate_and_materialize(log, &witness)
        .expect("State update is valid");

    // Test the `zk` execution.
    {
        let zk_storage = ZkStorage::new();
        let state = &mut WorkingSet::with_witness(zk_storage, witness);
        execute_module_logic::<ZkTestSpec>(state);
        test_state_update::<ZkTestSpec>(state);
    }
}

fn execute_module_logic<S: Spec>(state: &mut WorkingSet<S>) {
    let module = &mut module_c::ModuleC::<S>::default();
    module.execute("some_key", "some_value", state);
}

fn test_state_update<S: Spec>(state: &mut WorkingSet<S>) {
    let module = <module_c::ModuleC<S> as Default>::default();

    let expected_value = "some_value".to_owned();

    {
        let prefix = ModulePrefix::new_storage(
            "integration_tests::nested_modules::helpers::module_a",
            "ModuleA",
            "state_1_a",
        );
        let state_map = StateMap::<String, String>::new(prefix.into());
        let value = state_map.get(&"some_key".to_owned(), state).unwrap();

        assert_eq!(expected_value, value);
    }

    {
        let prefix = ModulePrefix::new_storage(
            "integration_tests::nested_modules::helpers::module_b",
            "ModuleB",
            "state_1_b",
        );
        let state_map = StateMap::<String, String>::new(prefix.into());
        let value = state_map.get(&"some_key".to_owned(), state).unwrap();

        assert_eq!(expected_value, value);
    }

    {
        let prefix = ModulePrefix::new_storage(
            "integration_tests::nested_modules::helpers::module_a",
            "ModuleA",
            "state_1_a",
        );
        let state_map = StateMap::<String, String>::new(prefix.into());
        let value = state_map.get(&"some_key".to_owned(), state).unwrap();

        assert_eq!(expected_value, value);
    }

    {
        let value = module.mod_1_a.state_2_a.get(state).unwrap();
        assert_eq!(expected_value, value);
    }
}
