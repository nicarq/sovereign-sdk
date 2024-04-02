use sov_modules_api::{
    AccessoryStateValue, CallResponse, Context, Module, ModuleError, ModuleId, ModuleInfo, Spec,
    WorkingSet,
};
use sov_prover_storage_manager::new_orphan_storage;
use sov_state::Storage;
use sov_test_utils::TestSpec;

#[derive(ModuleInfo)]
pub struct TestModule<S: Spec> {
    #[address]
    id: ModuleId,

    #[state]
    accessory_state: AccessoryStateValue<u32>,

    #[phantom]
    phantom: std::marker::PhantomData<S>,
}

impl<S: Spec> Module for TestModule<S> {
    type Spec = S;
    type Config = ();
    type CallMessage = ();
    type Event = ();

    fn genesis(
        &self,
        _config: &Self::Config,
        _working_set: &mut WorkingSet<S>,
    ) -> Result<(), ModuleError> {
        Ok(())
    }

    fn call(
        &self,
        _msg: Self::CallMessage,
        _context: &Context<Self::Spec>,
        _working_set: &mut WorkingSet<S>,
    ) -> Result<CallResponse, ModuleError> {
        unimplemented!()
    }
}

/// Check that:
/// 1. Accessory state does not change normal state root hash.
/// 2. Accessory state is saved to underlying the database.
/// 2. Accessory state is reverted together with normal state.
#[test]
fn test_accessory_value_setter() {
    let tmpdir = tempfile::tempdir().unwrap();
    let storage = new_orphan_storage(tmpdir.path()).unwrap();

    let module = TestModule::<TestSpec>::default();

    let mut ws1 = <WorkingSet<TestSpec>>::new(storage.clone());
    let mut ws2 = <WorkingSet<TestSpec>>::new(storage.clone());
    let mut ws3 = <WorkingSet<TestSpec>>::new(storage.clone());
    let mut ws4 = <WorkingSet<TestSpec>>::new(storage.clone());

    module.genesis(&(), &mut ws1).unwrap();

    let (reads_writes, _, witness) = ws1.checkpoint().0.freeze();
    let state_root_hash_initial = storage.validate_and_commit(reads_writes, &witness).unwrap();

    module.accessory_state.set(&42, &mut ws2.accessory_state());

    let checkpoint = ws2.checkpoint();
    let (reads_writes, accessory_writes, witness) = checkpoint.0.freeze();
    let state_root_hash_after = storage
        .validate_and_commit_with_accessory_update(
            reads_writes,
            &witness,
            accessory_writes.freeze(),
        )
        .unwrap();

    assert_eq!(state_root_hash_initial, state_root_hash_after);

    assert_eq!(
        42,
        module
            .accessory_state
            .get(&mut ws3.accessory_state())
            .unwrap(),
        "AccessoryStateValue read has returned an incorrect value"
    );

    module
        .accessory_state
        .set(&1000, &mut ws3.accessory_state());

    ws3.revert();

    assert_eq!(
        42,
        module
            .accessory_state
            .get(&mut ws4.accessory_state())
            .unwrap(),
        "AccessoryStateValue revert has failed"
    );
}
