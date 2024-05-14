mod versioned_value;

mod map;
mod value;
mod vec;

mod traits;
pub use map::{AccessoryStateMap, KernelStateMap, StateMap, StateMapError};
pub use traits::StateAccessor;
pub use value::{AccessoryStateValue, KernelStateValue, StateValue, StateValueError};
pub use vec::{AccessoryStateVec, KernelStateVec, StateVec};
pub use versioned_value::VersionedStateValue;

#[cfg(test)]
mod test {
    use sov_modules_core::namespaces::User;
    use sov_modules_core::{SlotKey, SlotValue, StateWriter, Storage, Version, WorkingSet};
    use sov_prover_storage_manager::SimpleStorageManager;
    use sov_test_utils::{TestSpec, TestStorageSpec as StorageSpec};

    #[derive(Clone)]
    struct TestCase {
        key: SlotKey,
        value: SlotValue,
        version: Version,
    }

    fn create_tests() -> Vec<TestCase> {
        vec![
            TestCase {
                key: SlotKey::from_slice(b"key_0"),
                value: SlotValue::from("value_0"),
                version: 1,
            },
            TestCase {
                key: SlotKey::from_slice(b"key_1"),
                value: SlotValue::from("value_1"),
                version: 2,
            },
            TestCase {
                key: SlotKey::from_slice(b"key_2"),
                value: SlotValue::from("value_2"),
                version: 3,
            },
            TestCase {
                key: SlotKey::from_slice(b"key_1"),
                value: SlotValue::from("value_3"),
                version: 4,
            },
        ]
    }

    #[test]
    fn test_jmt_storage() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tests = create_tests();
        {
            let mut storage_manager = SimpleStorageManager::new(tmpdir.path());
            for test in &tests {
                {
                    let storage = storage_manager.create_storage();
                    let mut working_set: WorkingSet<TestSpec> = WorkingSet::new(storage.clone());
                    StateWriter::<User>::set(&mut working_set, &test.key, test.value.clone());
                    let (checkpoint, _gas_meter, _) = working_set.checkpoint();
                    let (cache, _, witness) = checkpoint.freeze();
                    let (_, change_set) = storage
                        .validate_and_materialize(cache, &witness)
                        .expect("storage is valid");
                    storage_manager.commit(change_set);
                    let storage = storage_manager.create_storage();
                    assert_eq!(
                        Some(test.value.clone()),
                        storage.get::<User>(&test.key, None, &witness),
                        "Prover storage does not have correct value"
                    );
                }
            }
        }

        {
            let mut storage_manager = SimpleStorageManager::<StorageSpec>::new(tmpdir.path());
            let storage = storage_manager.create_storage();
            for test in tests {
                assert_eq!(
                    test.value,
                    storage
                        .get::<User>(&test.key, Some(test.version), &Default::default())
                        .unwrap()
                );
            }
        }
    }

    #[test]
    fn test_restart_lifecycle() {
        let tempdir = tempfile::tempdir().unwrap();
        let mut storage_manager = SimpleStorageManager::new(tempdir.path());
        {
            let storage = storage_manager.create_storage();
            assert!(storage.is_empty());
        }

        let key = SlotKey::from_slice(b"some_key");
        let value = SlotValue::from("some_value");
        // First restart
        {
            let storage = storage_manager.create_storage();
            assert!(storage.is_empty());
            let mut working_set: WorkingSet<TestSpec> = WorkingSet::new(storage.clone());
            StateWriter::<User>::set(&mut working_set, &key, value.clone());
            let (cache, _, witness) = working_set.checkpoint().0.freeze();
            let (_, change_set) = storage
                .validate_and_materialize(cache, &witness)
                .expect("storage is valid");
            storage_manager.commit(change_set);
        }

        // Correctly restart from disk
        {
            let storage = storage_manager.create_storage();
            assert_eq!(
                value,
                storage
                    .get::<User>(&key, None, &Default::default())
                    .unwrap()
            );
        }
    }
}
