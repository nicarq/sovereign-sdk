use sov_mock_da::MockDaSpec;
use sov_modules_api::namespaces::Kernel;
use sov_modules_core::capabilities::mocks::MockKernel;
use sov_modules_core::{
    Address, Context, KernelWorkingSet, SlotKey, SlotValue, StateCheckpoint, StateReaderAndWriter,
    WorkingSet,
};
use sov_prover_storage_manager::new_orphan_storage;
use sov_state::codec::BcsCodec;
use sov_test_utils::TestSpec;

#[test]
fn test_workingset_get() {
    let tempdir = tempfile::tempdir().unwrap();
    let codec = BcsCodec {};
    let storage = new_orphan_storage(tempdir.path()).unwrap();

    let prefix = sov_modules_core::Prefix::new(vec![1, 2, 3]);
    let storage_key = SlotKey::new(
        sov_modules_core::Namespace::User,
        &prefix,
        &vec![4, 5, 6],
        &codec,
    );
    let storage_value = SlotValue::new(&vec![7, 8, 9], &codec);

    let mut working_set = WorkingSet::<TestSpec>::new(storage.clone());
    working_set.set(&storage_key, storage_value.clone());

    assert_eq!(Some(storage_value), working_set.get(&storage_key));
}

#[test]
fn test_versioned_workingset_get() {
    let tempdir = tempfile::tempdir().unwrap();
    let codec = BcsCodec {};
    let storage = new_orphan_storage(tempdir.path()).unwrap();

    let prefix = sov_modules_core::Prefix::new(vec![1, 2, 3]);
    let storage_key = SlotKey::new(
        sov_modules_core::Namespace::User,
        &prefix,
        &vec![4, 5, 6],
        &codec,
    );
    let storage_value = SlotValue::new(&vec![7, 8, 9], &codec);

    let sender = Address::from([1; 32]);
    let sequencer = Address::from([2; 32]);
    let mut working_set = WorkingSet::<TestSpec>::new(storage.clone());
    let mut working_set =
        working_set.versioned_state(&Context::<TestSpec>::new(sender, sequencer, 1));
    working_set.set(&storage_key, storage_value.clone());

    assert_eq!(Some(storage_value), working_set.get(&storage_key));
}

#[test]
fn test_kernel_workingset_get() {
    let tempdir = tempfile::tempdir().unwrap();
    let codec = BcsCodec {};
    let storage = new_orphan_storage(tempdir.path()).unwrap();

    let prefix = sov_modules_core::Prefix::new(vec![1, 2, 3]);
    let storage_key = SlotKey::new(
        sov_modules_core::Namespace::User,
        &prefix,
        &vec![4, 5, 6],
        &codec,
    );
    let storage_value = SlotValue::new(&vec![7, 8, 9], &codec);
    let kernel: MockKernel<TestSpec, MockDaSpec> = MockKernel::new(4, 1);

    let mut working_set = StateCheckpoint::<TestSpec>::new(storage.clone());
    let mut working_set = KernelWorkingSet::from_kernel(&kernel, &mut working_set);

    StateReaderAndWriter::<Kernel>::set(&mut working_set, &storage_key, storage_value.clone());

    assert_eq!(
        Some(storage_value),
        StateReaderAndWriter::<Kernel>::get(&mut working_set, &storage_key)
    );
}
