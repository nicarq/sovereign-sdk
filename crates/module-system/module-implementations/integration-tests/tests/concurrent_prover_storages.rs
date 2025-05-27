use sov_modules_api::Spec;
use sov_rollup_interface::common::SlotNumber;
use sov_state::{
    Kernel, NativeStorage, OrderedReadsAndWrites, SlotKey, SlotValue, StateAccesses, StateUpdate,
    Storage, User,
};
use sov_test_utils::storage::{
    ForklessStorageManager, NativeStorageManager, NomtStorageManager, NonCommitingStorageManager,
    SimpleStorageManager,
};
use sov_test_utils::{TestNomtSpec, TestSpec};

#[test]
fn jmt_concurrent_prover_storages() {
    let storage_manager = SimpleStorageManager::new();
    concurrent_prover_storages::<TestSpec, _>(storage_manager, true);
}

#[test]
fn jmt_concurrent_prover_in_memory_storages() {
    let dir = tempfile::tempdir().unwrap();
    let inner_storage_manager = NativeStorageManager::new(dir.path()).unwrap();
    let storage_manager = NonCommitingStorageManager::new(dir, inner_storage_manager);
    concurrent_prover_storages::<TestSpec, _>(storage_manager, true);
}

#[test]
fn nomt_concurrent_prover_in_memory_storages() {
    let dir = tempfile::tempdir().unwrap();
    let inner_storage_manager = NomtStorageManager::new(dir.path()).unwrap();
    let storage_manager = NonCommitingStorageManager::new(dir, inner_storage_manager);
    // TODO: Enable root hashes assertion, after it is implemented
    concurrent_prover_storages::<TestNomtSpec, _>(storage_manager, false);
}

/// # Description
/// The test verifies that [`NativeStorage`] only returns data it has access to,
/// inclusive up to the latest version available when it was created.
/// It should not be able to see data at `next_version` or any future version passed as a parameter.
/// This test is important because of the leaky abstraction in `StorageManager`.
/// Data with a newer version can be written to the RocksDB,
/// while an instance of `ProverStorage` in the HTTP API hasn't been updated.
/// The HTTP API must serve consistent data during the request/response lifecycle,
/// even if data in RocksDB is being updated.
/// Notes:
///   - The test can only check and cover versioned data.
///     Non-versioned data, such as events, will leak into the HTTP API.
///
/// ## Test scenario
/// At each iteration, it creates storage and materializes changes.
/// Changes are performed on a single key in each namespace, and operations include updates and deletions.
/// Storage values are checked before and after data is written to disk.
/// Storages created in previous iterations are also checked before and after committing data to disk.
/// The test checks values in user, kernel, and accessory states.
/// For user and kernel states, it also checks that `get_with_proof` data is consistent with what is expected from the normal `get` method.
/// The test checks root hashes.
fn concurrent_prover_storages<S, Sm>(mut storage_manager: Sm, should_assert_root_hashes: bool)
where
    S: Spec,
    Sm: ForklessStorageManager<Storage = S::Storage>,
    S::Storage: NativeStorage,
    <S::Storage as Storage>::Root: Copy,
{
    let the_user_key = SlotKey::from_slice(b"user_key");
    let the_kernel_key = SlotKey::from_slice(b"kernel_key");
    let the_accessory_key = SlotKey::from_slice(b"accessory_key");

    let user_value_1 = SlotValue::from("user_value1");
    let user_value_2 = SlotValue::from("user_value2");
    let user_value_4 = SlotValue::from("user_value4");

    let kernel_value_1 = SlotValue::from("kernel_value1");
    let kernel_value_2 = SlotValue::from("kernel_value2");
    let kernel_value_4 = SlotValue::from("kernel_value4");

    let accessory_value_1 = SlotValue::from("accessory_value1");
    let accessory_value_2 = SlotValue::from("accessory_value2");
    let accessory_value_4 = SlotValue::from("accessory_value4");

    let expected_user_values_2 = vec![Some(user_value_1.clone())];
    let expected_user_values_3 = vec![Some(user_value_1.clone()), Some(user_value_2.clone())];
    let expected_user_values_4 = vec![Some(user_value_1.clone()), Some(user_value_2.clone()), None];
    let expected_user_values_5 = vec![
        Some(user_value_1.clone()),
        Some(user_value_2.clone()),
        None,
        Some(user_value_4.clone()),
    ];

    let expected_kernel_values_2 = vec![Some(kernel_value_1.clone())];
    let expected_kernel_values_3 = vec![Some(kernel_value_1.clone()), Some(kernel_value_2.clone())];
    let expected_kernel_values_4 = vec![
        Some(kernel_value_1.clone()),
        Some(kernel_value_2.clone()),
        None,
    ];
    let expected_kernel_values_5 = vec![
        Some(kernel_value_1.clone()),
        Some(kernel_value_2.clone()),
        None,
        Some(kernel_value_4.clone()),
    ];

    let expected_accessory_values_2 = vec![Some(accessory_value_1.clone())];
    let expected_accessory_values_3 = vec![
        Some(accessory_value_1.clone()),
        Some(accessory_value_2.clone()),
    ];
    let expected_accessory_values_4 = vec![
        Some(accessory_value_1.clone()),
        Some(accessory_value_2.clone()),
        None,
    ];
    let expected_accessory_values_5 = vec![
        Some(accessory_value_1.clone()),
        Some(accessory_value_2.clone()),
        None,
        Some(accessory_value_4.clone()),
    ];

    // Storage at version 1
    let (storage_1, genesis_root) = storage_manager.create_storage_with_root();
    let (root_1, change_set_1) = materialize_writes(
        storage_1,
        vec![(the_user_key.clone(), Some(user_value_1.clone()))],
        vec![(the_kernel_key.clone(), Some(kernel_value_1.clone()))],
        vec![(the_accessory_key.clone(), Some(accessory_value_1.clone()))],
        genesis_root,
    );
    let storage_1 = storage_manager.create_prover_storage();
    let assert_storage_1 = || {
        assert_values(
            &storage_1,
            &the_user_key,
            Vec::new(),
            ValueNamespace::StateUser,
        );
        assert_values(
            &storage_1,
            &the_kernel_key,
            Vec::new(),
            ValueNamespace::StateKernel,
        );
        assert_values(
            &storage_1,
            &the_accessory_key,
            Vec::new(),
            ValueNamespace::Accessory,
        );
        if should_assert_root_hashes {
            assert_root_hashes(&storage_1, Vec::new());
        }
    };
    assert_storage_1();
    storage_manager.commit_change_set(change_set_1, root_1);

    // Storage at version 2
    let storage_2 = storage_manager.create_prover_storage();
    let (root_2, change_set_2) = materialize_writes(
        storage_2,
        vec![(the_user_key.clone(), Some(user_value_2.clone()))],
        vec![(the_kernel_key.clone(), Some(kernel_value_2.clone()))],
        vec![(the_accessory_key.clone(), Some(accessory_value_2.clone()))],
        root_1,
    );
    let storage_2 = storage_manager.create_prover_storage();
    let assert_storage_2 = || {
        assert_values(
            &storage_2,
            &the_user_key,
            expected_user_values_2.clone(),
            ValueNamespace::StateUser,
        );
        assert_values(
            &storage_2,
            &the_kernel_key,
            expected_kernel_values_2.clone(),
            ValueNamespace::StateKernel,
        );
        assert_values(
            &storage_2,
            &the_accessory_key,
            expected_accessory_values_2.clone(),
            ValueNamespace::Accessory,
        );
        if should_assert_root_hashes {
            assert_root_hashes(&storage_2, vec![root_1]);
        }
    };
    assert_storage_1();
    assert_storage_2();
    storage_manager.commit_change_set(change_set_2, root_2);
    assert_storage_1();
    assert_storage_2();

    // Storage at version 3
    let storage_3 = storage_manager.create_prover_storage();
    let (root_3, change_set_3) = materialize_writes(
        storage_3,
        vec![(the_user_key.clone(), None)],
        vec![(the_kernel_key.clone(), None)],
        vec![(the_accessory_key.clone(), None)],
        root_2,
    );
    let storage_3 = storage_manager.create_prover_storage();
    let assert_storage_3 = || {
        assert_values(
            &storage_3,
            &the_user_key,
            expected_user_values_3.clone(),
            ValueNamespace::StateUser,
        );
        assert_values(
            &storage_3,
            &the_kernel_key,
            expected_kernel_values_3.clone(),
            ValueNamespace::StateKernel,
        );
        assert_values(
            &storage_3,
            &the_accessory_key,
            expected_accessory_values_3.clone(),
            ValueNamespace::Accessory,
        );
        if should_assert_root_hashes {
            assert_root_hashes(&storage_3, vec![root_1, root_2]);
        }
    };
    assert_storage_1();
    assert_storage_2();
    assert_storage_3();
    storage_manager.commit_change_set(change_set_3, root_3);
    assert_storage_1();
    assert_storage_2();
    assert_storage_3();

    // Storage at version 4
    let storage_4 = storage_manager.create_prover_storage();
    let (root_4, change_set_4) = materialize_writes(
        storage_4,
        vec![(the_user_key.clone(), Some(user_value_4.clone()))],
        vec![(the_kernel_key.clone(), Some(kernel_value_4.clone()))],
        vec![(the_accessory_key.clone(), Some(accessory_value_4.clone()))],
        root_3,
    );
    let storage_4 = storage_manager.create_prover_storage();
    let assert_storage_4 = || {
        assert_values(
            &storage_4,
            &the_user_key,
            expected_user_values_4.clone(),
            ValueNamespace::StateUser,
        );
        assert_values(
            &storage_4,
            &the_kernel_key,
            expected_kernel_values_4.clone(),
            ValueNamespace::StateKernel,
        );
        assert_values(
            &storage_4,
            &the_accessory_key,
            expected_accessory_values_4.clone(),
            ValueNamespace::Accessory,
        );
        if should_assert_root_hashes {
            assert_root_hashes(&storage_4, vec![root_1, root_2, root_3]);
        }
    };
    assert_storage_1();
    assert_storage_2();
    assert_storage_3();
    assert_storage_4();
    storage_manager.commit_change_set(change_set_4, root_4);
    assert_storage_1();
    assert_storage_2();
    assert_storage_3();
    assert_storage_4();
    // Check that all previous values are available
    let storage_5 = storage_manager.create_prover_storage();
    assert_values(
        &storage_5,
        &the_user_key,
        expected_user_values_5.clone(),
        ValueNamespace::StateUser,
    );
    assert_values(
        &storage_5,
        &the_kernel_key,
        expected_kernel_values_5.clone(),
        ValueNamespace::StateKernel,
    );
    assert_values(
        &storage_5,
        &the_accessory_key,
        expected_accessory_values_5.clone(),
        ValueNamespace::Accessory,
    );
    if should_assert_root_hashes {
        assert_root_hashes(&storage_5, vec![root_1, root_2, root_3, root_4]);
    }
}

fn materialize_writes<S: Storage>(
    storage: S,
    user_writes: Vec<(SlotKey, Option<SlotValue>)>,
    kernel_writes: Vec<(SlotKey, Option<SlotValue>)>,
    accessory_writes: Vec<(SlotKey, Option<SlotValue>)>,
    prev_root: S::Root,
) -> (S::Root, S::ChangeSet) {
    let state_accesses = StateAccesses {
        user: OrderedReadsAndWrites {
            ordered_writes: user_writes,
            ..Default::default()
        },
        kernel: OrderedReadsAndWrites {
            ordered_writes: kernel_writes,
            ..Default::default()
        },
    };

    let (root, mut state_update) = storage
        .compute_state_update(state_accesses, &S::Witness::default(), prev_root)
        .unwrap();

    state_update.add_accessory_items(accessory_writes);

    (root, storage.materialize_changes(state_update))
}

#[derive(Debug, Clone, Copy)]
enum ValueNamespace {
    StateKernel,
    StateUser,
    Accessory,
}

/// Checks that given storage can see all expected values for a given key.
/// The first element in expected_values is supposed to be rollup_height == 0
/// Last element checked against "last" version (None parameter)
/// get_with_proof is also checked for User and Kernel namespaces.
fn assert_values<S: NativeStorage>(
    storage: &S,
    key: &SlotKey,
    expected_values: Vec<Option<SlotValue>>,
    namespace: ValueNamespace,
) {
    let witness_stub = S::Witness::default();
    let get_value = |version: Option<SlotNumber>| -> Option<SlotValue> {
        match namespace {
            ValueNamespace::StateKernel => {
                let just_value = storage.get::<Kernel>(key, version, &witness_stub);
                let with_proof = storage
                    .get_with_proof::<Kernel>(key.clone(), version)
                    .ok()
                    .and_then(|with_proof| with_proof.value);
                // Assume that proof and the rest are correct
                assert_eq!(just_value, with_proof);
                just_value
            }
            ValueNamespace::StateUser => {
                let just_value = storage.get::<User>(key, version, &witness_stub);
                let with_proof = storage
                    .get_with_proof::<User>(key.clone(), version)
                    .ok()
                    .and_then(|with_proof| with_proof.value);
                assert_eq!(just_value, with_proof);
                just_value
            }
            ValueNamespace::Accessory => storage.get_accessory(key, version),
        }
    };
    let last_value = expected_values.last().unwrap_or(&None).clone();

    // No version is equal to the last expected value
    assert_eq!(
        last_value,
        get_value(None),
        "Not specifying version should be equal to last version for this storage in {:?}",
        namespace,
    );

    let next_version = expected_values.len() as u64;
    for (idx, expected_value) in expected_values.into_iter().enumerate() {
        let version = SlotNumber::new_dangerous(idx as u64);
        assert_eq!(expected_value, get_value(Some(version)));
    }

    // Future versions are not available
    // Checking 3 more next versions for extra confidence
    for version in next_version..(next_version + 3) {
        let version = SlotNumber::new_dangerous(version);

        assert_eq!(
            None,
            get_value(Some(version)),
            "Future version({}) should not be available",
            version
        );
    }

    // Boundary check
    assert_eq!(
        None,
        get_value(Some(SlotNumber::MAX)),
        "Future version(u64::MAX) should not be available",
    );
}

fn assert_root_hashes<S: NativeStorage>(storage: &S, expected_root_hashes: Vec<S::Root>) {
    let next_version = expected_root_hashes.len() as u64;
    for (version, expected_root_hash) in expected_root_hashes.into_iter().enumerate() {
        assert_eq!(
            expected_root_hash,
            storage
                .get_root_hash(SlotNumber::new_dangerous(version as u64))
                .unwrap()
        );
    }
    let future_root = storage
        .get_root_hash(SlotNumber::new_dangerous(next_version))
        .unwrap_err();
    let expected_error = format!("Root node not found for version {}.", next_version);
    assert_eq!(expected_error, future_root.to_string());
}
