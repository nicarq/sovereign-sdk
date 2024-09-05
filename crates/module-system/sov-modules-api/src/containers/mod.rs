mod versioned_value;

pub(crate) mod map;
pub(crate) mod value;
pub(crate) mod vec;
pub(crate) mod versioned_vec;

pub use map::{AccessoryStateMap, KernelStateMap, StateMap, StateMapError};
pub use value::{AccessoryStateValue, KernelStateValue, StateValue, StateValueError};
pub use vec::{AccessoryStateVec, KernelStateVec, StateVec};
pub use versioned_value::VersionedStateValue;
pub use versioned_vec::VersionedStateVec;

#[cfg(test)]
mod test {

    use sov_mock_zkvm::MockZkVerifier;
    use sov_state::namespaces::User;
    use sov_state::{DefaultStorageSpec, SlotKey, SlotValue, Storage};
    use sov_test_utils::storage::SimpleStorageManager;

    use crate::capabilities::mocks::MockKernel;
    use crate::execution_mode::Native;
    use crate::{CryptoSpec, StateWriter, Version, WorkingSet};

    type StorageSpec = DefaultStorageSpec<TestHasher>;
    type TestSpec = crate::default_spec::DefaultSpec<MockZkVerifier, MockZkVerifier, Native>;
    type TestHasher = <<TestSpec as crate::Spec>::CryptoSpec as CryptoSpec>::Hasher;

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
    fn test_jmt_storage() -> anyhow::Result<()> {
        let tmpdir = tempfile::tempdir().unwrap();
        let tests = create_tests();
        {
            let mut storage_manager = SimpleStorageManager::new(tmpdir.path());
            let mut kernel = MockKernel::<TestSpec>::default();
            for test in &tests {
                {
                    let storage = storage_manager.create_storage();
                    let mut working_set: WorkingSet<TestSpec> =
                        WorkingSet::new_deprecated(storage.clone(), &kernel);
                    StateWriter::<User>::set(&mut working_set, &test.key, test.value.clone())?;
                    let (checkpoint, _gas_meter, _) = working_set.checkpoint();
                    let (cache, _, witness) = checkpoint.freeze();
                    let (_, change_set) = storage
                        .validate_and_materialize(cache, &witness)
                        .expect("storage is valid");
                    storage_manager.commit(change_set);
                    kernel.increase_heights();
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
            let storage_manager = SimpleStorageManager::<StorageSpec>::new(tmpdir.path());
            let storage = storage_manager.create_storage();
            for test in tests {
                assert_eq!(
                    Some(test.value),
                    storage.get::<User>(&test.key, Some(test.version), &Default::default())
                );
            }
        }

        Ok(())
    }

    #[test]
    fn test_restart_lifecycle() -> anyhow::Result<()> {
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
            let mut working_set: WorkingSet<TestSpec> =
                WorkingSet::new_deprecated(storage.clone(), &MockKernel::<TestSpec>::default());
            StateWriter::<User>::set(&mut working_set, &key, value.clone())?;
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
                Some(value),
                storage.get::<User>(&key, None, &Default::default())
            );
        }

        Ok(())
    }
}
