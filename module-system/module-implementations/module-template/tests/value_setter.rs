use module_template::{CallMessage, Event, ExampleModule, ExampleModuleConfig, Response};
use sov_modules_api::{Address, Context, Module, Spec, WorkingSet};
use sov_prover_storage_manager::new_orphan_storage;
use sov_state::{DefaultStorageSpec, ZkStorage};
use sov_test_utils::{TestSpec, ZkTestSpec};

#[test]
fn test_value_setter() {
    let tmpdir = tempfile::tempdir().unwrap();

    let storage = new_orphan_storage::<DefaultStorageSpec>(tmpdir.path()).unwrap();
    let mut working_set = WorkingSet::new(storage);

    let admin = Address::from([1; 32]);
    let sequencer = Address::from([2; 32]);

    // Test Native-Context
    {
        let config = ExampleModuleConfig {};
        let context = Context::<TestSpec>::new(admin, sequencer, 1);
        test_value_setter_helper(context, &config, &mut working_set);
    }

    let (_, _, witness) = working_set.checkpoint().0.freeze();

    // Test Zk-Context
    {
        let config = ExampleModuleConfig {};
        let zk_context = Context::<ZkTestSpec>::new(admin, sequencer, 1);
        let mut zk_working_set = WorkingSet::with_witness(ZkStorage::new(), witness);
        test_value_setter_helper(zk_context, &config, &mut zk_working_set);
    }
}

fn test_value_setter_helper<S: Spec>(
    context: Context<S>,
    config: &ExampleModuleConfig,
    working_set: &mut WorkingSet<S>,
) {
    let module = ExampleModule::<S>::default();
    module.genesis(config, working_set).unwrap();

    let new_value = 99;
    let call_msg = CallMessage::SetValue(new_value);

    // Test events
    {
        module.call(call_msg, &context, working_set).unwrap();
        let typed_event = working_set.take_event(0).unwrap();
        assert_eq!(
            typed_event.downcast::<Event>().unwrap(),
            Event::Set { value: 99 }
        );
    }

    // Test query
    #[cfg(feature = "native")]
    {
        let query_response = module.query_value(working_set);
        assert_eq!(
            Response {
                value: Some(new_value)
            },
            query_response
        );
    }
}
