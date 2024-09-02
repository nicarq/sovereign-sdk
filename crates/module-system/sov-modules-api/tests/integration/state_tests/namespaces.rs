use std::convert::Infallible;

use sov_modules_api::{
    KernelStateValue, KernelWorkingSet, StateCheckpoint, StateMap, StateValue,
    VersionedStateReadWriter, VersionedStateValue, WorkingSet,
};
use sov_state::{Prefix, ProvableNamespace};
use sov_test_utils::storage::SimpleStorageManager;

use crate::state_tests::{commit_to_storage, StorageSpec, S};

/// Test that the state values with a standard working set get written to the user space
#[test]
fn test_state_value_user_namespace() -> Result<(), Infallible> {
    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = SimpleStorageManager::<StorageSpec>::new(tmpdir.path());
    let storage = storage_manager.create_storage();

    let state_value = StateValue::new(Prefix::new(vec![0]));

    // Native execution
    let mut state: StateCheckpoint<S> = StateCheckpoint::new(storage.clone());
    state_value.set(&11, &mut state)?;
    let _ = state_value.get(&mut state);
    state_value.set(&22, &mut state)?;

    let storage = commit_to_storage(state, storage, &mut storage_manager);

    // In the first version the user and the kernel root hashes are the same
    let kernel_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::Kernel, 0)
        .unwrap();
    let user_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::User, 0)
        .unwrap();
    assert_eq!(kernel_root_hash, user_root_hash);

    // Then the kernel is the same but the user root hash changes
    let new_kernel_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::Kernel, 1)
        .unwrap();
    let new_user_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::User, 1)
        .unwrap();
    assert_eq!(kernel_root_hash, new_kernel_root_hash);
    assert_ne!(new_kernel_root_hash, new_user_root_hash);

    Ok(())
}

/// Test that the state values with a kernel working set get written to the kernel space
#[test]
fn test_state_value_kernel_namespace() -> Result<(), Infallible> {
    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = SimpleStorageManager::<StorageSpec>::new(tmpdir.path());
    let storage = storage_manager.create_storage();

    let state_value = KernelStateValue::new(Prefix::new(vec![0]));

    // Native execution
    let mut state: StateCheckpoint<S> = StateCheckpoint::new(storage.clone());

    let mut kernel_working_set = KernelWorkingSet::uninitialized(&mut state);
    state_value.set(&11, &mut kernel_working_set)?;
    let _ = state_value.get(&mut kernel_working_set);
    state_value.set(&22, &mut kernel_working_set)?;

    let storage = commit_to_storage(state, storage, &mut storage_manager);

    // In the first version the user and the kernel root hashes are the same
    let kernel_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::Kernel, 0)
        .unwrap();
    let user_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::User, 0)
        .unwrap();
    assert_eq!(kernel_root_hash, user_root_hash);

    // Then the kernel is the same but the user root hash changes
    let new_kernel_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::Kernel, 1)
        .unwrap();
    let new_user_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::User, 1)
        .unwrap();
    assert_eq!(user_root_hash, new_user_root_hash);
    assert_ne!(new_kernel_root_hash, new_user_root_hash);

    Ok(())
}

/// Test that the state maps with a standard working set get written to the user space
#[test]
fn test_state_map_user_namespace() -> Result<(), Infallible> {
    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = SimpleStorageManager::<StorageSpec>::new(tmpdir.path());
    let storage = storage_manager.create_storage();

    let state_value = StateMap::new(Prefix::new(vec![0]));

    // Native execution
    let mut state: StateCheckpoint<S> = StateCheckpoint::new(storage.clone());
    state_value.set(&11, &0, &mut state)?;
    let _ = state_value.get(&0, &mut state);
    state_value.set(&22, &0, &mut state)?;

    let storage = commit_to_storage(state, storage, &mut storage_manager);

    // In the first version the user and the kernel root hashes are the same
    let kernel_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::Kernel, 0)
        .unwrap();
    let user_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::User, 0)
        .unwrap();
    assert_eq!(kernel_root_hash, user_root_hash);

    // Then the kernel is the same but the user root hash changes
    let new_kernel_root_hash: sov_state::jmt::RootHash = storage
        .get_root_hash_namespace(ProvableNamespace::Kernel, 1)
        .unwrap();
    let new_user_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::User, 1)
        .unwrap();
    assert_eq!(kernel_root_hash, new_kernel_root_hash);
    assert_ne!(new_kernel_root_hash, new_user_root_hash);

    Ok(())
}

/// Test that the kernel state maps with a kernel working set get written to the kernel space
#[test]
fn test_versioned_state_value_kernel_namespace() -> Result<(), Infallible> {
    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = SimpleStorageManager::<StorageSpec>::new(tmpdir.path());
    let storage = storage_manager.create_storage();

    let state_value = VersionedStateValue::new(Prefix::new(vec![0]));

    // Native execution
    let working_set: WorkingSet<S> = WorkingSet::new_deprecated(storage.clone());

    let mut state = working_set.checkpoint().0;
    let mut kernel_working_set = KernelWorkingSet::uninitialized(&mut state);
    state_value.set_true_current(&11, &mut kernel_working_set);
    let _ = state_value.get_current(&mut kernel_working_set);
    state_value.set_true_current(&22, &mut kernel_working_set);

    let storage = commit_to_storage(state, storage, &mut storage_manager);

    // In the first version the user and the kernel root hashes are the same
    let kernel_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::Kernel, 0)
        .unwrap();
    let user_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::User, 0)
        .unwrap();
    assert_eq!(kernel_root_hash, user_root_hash);

    // Then the kernel is the same but the user root hash changes
    let new_kernel_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::Kernel, 1)
        .unwrap();
    let new_user_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::User, 1)
        .unwrap();
    assert_eq!(user_root_hash, new_user_root_hash);
    assert_ne!(new_kernel_root_hash, new_user_root_hash);

    // Check that we can get the current value with a standard working set
    let working_set: WorkingSet<S> = WorkingSet::new_deprecated(storage.clone());
    let mut state_checkpoint = working_set.checkpoint().0;
    let kernel_working_set = &mut KernelWorkingSet::uninitialized(&mut state_checkpoint);
    let mut versioned_reader = VersionedStateReadWriter::from_kernel_ws_virtual(kernel_working_set);
    let val = state_value
        .get_current(&mut versioned_reader)?
        .expect("We should be able to retrieve the state value");

    assert_eq!(val, 22);

    Ok(())
}
