use std::convert::Infallible;

use sov_modules_api::capabilities::mocks::MockKernel;
use sov_modules_api::capabilities::Kernel;
use sov_modules_api::{
    KernelStateValue, StateCheckpoint, StateMap, StateValue, VersionedStateValue,
};
use sov_rollup_interface::common::IntoSlotNumber;
use sov_state::{BorshCodec, Prefix, ProvableNamespace};
use sov_test_utils::storage::SimpleStorageManager;

use crate::state_tests::{commit_to_storage, StorageSpec, S};

/// Test that the state values with a standard working set get written to the user space
#[test]
fn test_state_value_user_namespace() -> Result<(), Infallible> {
    let mut storage_manager = SimpleStorageManager::<StorageSpec>::new();
    let storage = storage_manager.create_storage();

    let mut state_value = StateValue::with_codec(Prefix::new(vec![0]), BorshCodec);

    let mut kernel = MockKernel::<S>::default();

    // Native execution
    let mut state: StateCheckpoint<S> = StateCheckpoint::new(storage.clone(), &kernel);
    state_value.set(&11, &mut state)?;
    let storage = commit_to_storage(state, storage, &mut kernel, &mut storage_manager);

    // In the first version the user and the kernel root hashes are different
    let kernel_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::Kernel, 0.to_slot_number())
        .unwrap();
    let user_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::User, 0.to_slot_number())
        .unwrap();
    assert_ne!(kernel_root_hash, user_root_hash);

    let mut state: StateCheckpoint<S> = StateCheckpoint::new(storage.clone(), &kernel);
    let _ = state_value.get(&mut state);
    state_value.set(&22, &mut state)?;
    let storage = commit_to_storage(state, storage, &mut kernel, &mut storage_manager);

    // Then the kernel is the same but the user root hash changes
    let new_kernel_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::Kernel, 1.to_slot_number())
        .unwrap();
    let new_user_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::User, 1.to_slot_number())
        .unwrap();
    assert_eq!(kernel_root_hash, new_kernel_root_hash);
    assert_ne!(new_kernel_root_hash, new_user_root_hash);

    Ok(())
}

/// Test that the state values with a kernel working set get written to the kernel space
#[test]
fn test_state_value_kernel_namespace() -> Result<(), Infallible> {
    let mut storage_manager = SimpleStorageManager::<StorageSpec>::new();
    let storage = storage_manager.create_storage();

    let mut kernel = MockKernel::<S>::default();

    let mut state_value = KernelStateValue::with_codec(Prefix::new(vec![0]), BorshCodec);

    // Native execution
    let mut state: StateCheckpoint<S> = StateCheckpoint::new(storage.clone(), &kernel);
    let mut kernel_working_set = kernel.accessor(&mut state);
    state_value.set(&11, &mut kernel_working_set)?;

    let storage = commit_to_storage(state, storage, &mut kernel, &mut storage_manager);

    // In the first version the user and the kernel root hashes are different
    let kernel_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::Kernel, 0.to_slot_number())
        .unwrap();
    let user_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::User, 0.to_slot_number())
        .unwrap();
    assert_ne!(kernel_root_hash, user_root_hash);

    let mut state: StateCheckpoint<S> = StateCheckpoint::new(storage.clone(), &kernel);
    let mut kernel_working_set = kernel.accessor(&mut state);
    let _ = state_value.get(&mut kernel_working_set);
    state_value.set(&22, &mut kernel_working_set)?;
    let storage = commit_to_storage(state, storage, &mut kernel, &mut storage_manager);

    // Then the kernel is the same, but the user root hash changes
    let new_kernel_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::Kernel, 1.to_slot_number())
        .unwrap();
    let new_user_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::User, 1.to_slot_number())
        .unwrap();
    assert_eq!(user_root_hash, new_user_root_hash);
    assert_ne!(new_kernel_root_hash, new_user_root_hash);

    Ok(())
}

/// Test that the state maps with a standard working set get written to the user space
#[test]
fn test_state_map_user_namespace() -> Result<(), Infallible> {
    let mut storage_manager = SimpleStorageManager::<StorageSpec>::new();
    let storage = storage_manager.create_storage();

    let mut state_value = StateMap::with_codec(Prefix::new(vec![0]), BorshCodec);
    let mut kernel = MockKernel::<S>::default();

    // Native execution
    let mut state: StateCheckpoint<S> = StateCheckpoint::new(storage.clone(), &kernel);
    state_value.set(&11, &0, &mut state)?;

    // Committing data at height 0
    let storage = commit_to_storage(state, storage, &mut kernel, &mut storage_manager);

    let kernel_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::Kernel, 0.to_slot_number())
        .unwrap();
    let user_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::User, 0.to_slot_number())
        .unwrap();
    // In the first version the user and the kernel root hashes are different
    assert_ne!(kernel_root_hash, user_root_hash);

    let mut state: StateCheckpoint<S> = StateCheckpoint::new(storage.clone(), &kernel);
    state_value.set(&11, &0, &mut state)?;
    let _ = state_value.get(&0, &mut state);
    state_value.set(&22, &0, &mut state)?;
    // Committing at height = 1
    let storage = commit_to_storage(state, storage, &mut kernel, &mut storage_manager);

    // Then the kernel is the same but the user root hash changes
    let new_kernel_root_hash: sov_state::jmt::RootHash = storage
        .get_root_hash_namespace(ProvableNamespace::Kernel, 1.to_slot_number())
        .unwrap();
    let new_user_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::User, 1.to_slot_number())
        .unwrap();
    assert_eq!(kernel_root_hash, new_kernel_root_hash);
    assert_ne!(user_root_hash, new_user_root_hash);

    Ok(())
}

/// Test that the kernel state maps with a kernel working set get written to the kernel space
#[test]
fn test_versioned_state_value_kernel_namespace() -> Result<(), Infallible> {
    let mut storage_manager = SimpleStorageManager::<StorageSpec>::new();
    let storage = storage_manager.create_storage();

    let mut state_value = VersionedStateValue::with_codec(Prefix::new(vec![0]), BorshCodec);

    let mut kernel = MockKernel::<S>::default();

    // Native execution
    let mut state = StateCheckpoint::new(storage.clone(), &kernel);
    let mut kernel_working_set = kernel.accessor(&mut state);
    state_value
        .set_true_current(&11, &mut kernel_working_set)
        .unwrap();

    let storage = commit_to_storage(state, storage, &mut kernel, &mut storage_manager);

    // In the first version the user and the kernel root hashes are different from one another
    let kernel_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::Kernel, 0.to_slot_number())
        .unwrap();
    let user_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::User, 0.to_slot_number())
        .unwrap();
    assert_ne!(kernel_root_hash, user_root_hash);

    let mut state = StateCheckpoint::new(storage.clone(), &kernel);
    let mut kernel_working_set = kernel.accessor(&mut state);
    let _ = state_value.get_current(&mut kernel_working_set);
    state_value
        .set_true_current(&22, &mut kernel_working_set)
        .unwrap();
    let storage = commit_to_storage(state, storage, &mut kernel, &mut storage_manager);

    // Then the kernel is the same but the user root hash changes
    let new_kernel_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::Kernel, 1.to_slot_number())
        .unwrap();
    let new_user_root_hash = storage
        .get_root_hash_namespace(ProvableNamespace::User, 1.to_slot_number())
        .unwrap();
    assert_eq!(user_root_hash, new_user_root_hash);
    assert_ne!(new_kernel_root_hash, new_user_root_hash);

    // Check that we can get the current value with a standard working set
    let mut kernel_reset = MockKernel::<S>::default();
    let mut state = StateCheckpoint::new(storage.clone(), &kernel_reset);
    let val_0 = state_value
        .get_current(&mut state)?
        .expect("We should be able to retrieve the state value");
    assert_eq!(val_0, 11);

    kernel_reset.increase_heights();

    let mut state = StateCheckpoint::new(storage.clone(), &kernel_reset);
    let val_0 = state_value
        .get_current(&mut state)?
        .expect("We should be able to retrieve the state value");
    assert_eq!(val_0, 22);

    Ok(())
}
