use sov_modules_api::{Address, Context, Module, Spec, WorkingSet};
use sov_prover_storage_manager::new_orphan_storage;
use sov_state::ZkStorage;
use sov_test_utils::{TestSpec, ZkTestSpec};

use super::{Event, ValueSetter};
use crate::{call, ValueSetterConfig};

#[test]
fn test_value_setter() {
    let tmpdir = tempfile::tempdir().unwrap();
    let mut working_set = WorkingSet::new(new_orphan_storage(tmpdir.path()).unwrap());
    let admin = Address::from([1; 32]);
    let sequencer = Address::from([2; 32]);
    // Test Native-Context
    #[cfg(feature = "native")]
    {
        let config = ValueSetterConfig { admin };
        let context = Context::<TestSpec>::new(admin, Default::default(), sequencer, 1);
        test_value_setter_helper(context, &config, &mut working_set);
    }

    let (_, _, witness) = working_set.checkpoint().0.freeze();

    // Test Zk-Context
    {
        let config = ValueSetterConfig { admin };
        let zk_context = Context::<ZkTestSpec>::new(admin, Default::default(), sequencer, 1);
        let mut zk_working_set = WorkingSet::with_witness(ZkStorage::new(), witness);
        test_value_setter_helper(zk_context, &config, &mut zk_working_set);
    }
}

fn test_value_setter_helper<S: Spec>(
    context: Context<S>,
    config: &ValueSetterConfig<S>,
    state: &mut WorkingSet<S>,
) {
    let module = ValueSetter::<S>::default();
    module.genesis(config, state).unwrap();

    let new_value = 99;
    let call_msg = call::CallMessage::SetValue(new_value);

    // Test events
    {
        module.call(call_msg, &context, state).unwrap();
        let typed_event = state.take_event(0).unwrap();
        assert_eq!(
            typed_event.downcast::<Event>().unwrap(),
            Event::NewValue(99)
        );
    }

    // Test query
    assert_eq!(module.value.get(state), Some(new_value));
}

#[test]
fn test_err_on_sender_is_not_admin() {
    let sender = Address::from([1; 32]);
    let sequencer = Address::from([2; 32]);

    let tmpdir = tempfile::tempdir().unwrap();
    let storage = new_orphan_storage(tmpdir.path()).unwrap();
    let mut prover_working_set = WorkingSet::new(storage);

    let sender_not_admin = Address::from([2; 32]);
    // Test Prover-Context
    {
        let config = ValueSetterConfig {
            admin: sender_not_admin,
        };
        let context = Context::<TestSpec>::new(sender, Default::default(), sequencer, 1);
        test_err_on_sender_is_not_admin_helper(context, &config, &mut prover_working_set);
    }
    let (_, _, witness) = prover_working_set.checkpoint().0.freeze();

    // Test Zk-Context
    {
        let config = ValueSetterConfig {
            admin: sender_not_admin,
        };
        let zk_backing_store = ZkStorage::new();
        let zk_context = Context::<ZkTestSpec>::new(sender, Default::default(), sequencer, 1);
        let zk_working_set = &mut WorkingSet::with_witness(zk_backing_store, witness);
        test_err_on_sender_is_not_admin_helper(zk_context, &config, zk_working_set);
    }
}

fn test_err_on_sender_is_not_admin_helper<S: Spec>(
    context: Context<S>,
    config: &ValueSetterConfig<S>,
    state: &mut WorkingSet<S>,
) {
    let module = ValueSetter::<S>::default();
    module.genesis(config, state).unwrap();
    let resp = module.set_value(11, &context, state);

    assert!(resp.is_err());
}
