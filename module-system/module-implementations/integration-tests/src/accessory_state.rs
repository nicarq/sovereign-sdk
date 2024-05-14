use sov_modules_api::{
    AccessoryStateValue, CallResponse, Context, Module, ModuleError, ModuleId, ModuleInfo, Spec,
    TxState, WorkingSet,
};
use sov_prover_storage_manager::SimpleStorageManager;
use sov_state::Storage;
use sov_test_utils::{TestSpec, TestStorageSpec as StorageSpec};

#[derive(ModuleInfo)]
pub struct TestModule<S: Spec> {
    #[id]
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
        _working_set: &mut impl TxState<S>,
    ) -> Result<CallResponse, ModuleError> {
        unimplemented!()
    }
}

/// Check that:
/// 1. Accessory state does not change normal state root hash.
/// 2. Accessory state is reverted together with normal state.
/// Changes are returned explicitly by storage trait.
#[test]
fn test_accessory_value_setter() {
    let module = TestModule::<TestSpec>::default();

    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = SimpleStorageManager::<StorageSpec>::new(tmpdir.path());

    // 0. Genesis
    let storage = storage_manager.create_storage();
    let mut ws = <WorkingSet<TestSpec>>::new(storage.clone());

    module.genesis(&(), &mut ws).unwrap();

    let (reads_writes, _, witness) = ws.checkpoint().0.freeze();
    let (state_root_hash_initial, change_set_genesis) = storage
        .validate_and_materialize(reads_writes, &witness)
        .unwrap();
    storage_manager.commit(change_set_genesis);

    // 1. Check that root hash is not changed after
    let storage = storage_manager.create_storage();
    let mut ws = <WorkingSet<TestSpec>>::new(storage.clone());

    module.accessory_state.set(&42, &mut ws);

    let checkpoint = ws.checkpoint();
    let (state_writes, accessory_writes, witness) = checkpoint.0.freeze();
    let (state_root_hash_after, change_set_after) = storage
        .validate_and_materialize_with_accessory_update(
            state_writes,
            &witness,
            accessory_writes.freeze(),
        )
        .unwrap();

    assert_eq!(
        state_root_hash_initial, state_root_hash_after,
        "State root has been changed by accessory writes"
    );

    storage_manager.commit(change_set_after);
    let storage = storage_manager.create_storage();
    let mut ws = <WorkingSet<TestSpec>>::new(storage.clone());

    assert_eq!(
        42,
        module.accessory_state.get(&mut ws).unwrap(),
        "AccessoryStateValue read has returned an incorrect value"
    );

    module.accessory_state.set(&1000, &mut ws);

    let (checkpoint, _gas_meter) = ws.revert();
    let (state_writes, accessory_writes, witness) = checkpoint.freeze();
    let (state_root_hash_after, change_set_after) = storage
        .validate_and_materialize_with_accessory_update(
            state_writes,
            &witness,
            accessory_writes.freeze(),
        )
        .unwrap();

    assert_eq!(
        state_root_hash_initial, state_root_hash_after,
        "State root has been changed by accessory revert"
    );

    storage_manager.commit(change_set_after);
    let storage = storage_manager.create_storage();
    let mut ws = WorkingSet::<TestSpec>::new(storage.clone());

    assert_eq!(
        42,
        module.accessory_state.get(&mut ws).unwrap(),
        "AccessoryStateValue revert has failed"
    );
}
