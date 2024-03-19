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
    use sov_mock_da::{MockBlockHeader, MockDaSpec};
    use sov_modules_core::namespaces::User;
    use sov_modules_core::{
        SlotKey, SlotValue, StateReaderAndWriter, Storage, Version, WorkingSet,
    };
    use sov_prover_storage_manager::ProverStorageManager;
    use sov_rollup_interface::storage::HierarchicalStorageManager;
    use sov_state::DefaultStorageSpec;
    use sov_test_utils::TestSpec;

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
        let tempdir = tempfile::tempdir().unwrap();
        let tests = create_tests();
        let storage_config = sov_state::config::Config {
            path: tempdir.path().to_path_buf(),
        };
        {
            let mut storage_manager =
                ProverStorageManager::<MockDaSpec, DefaultStorageSpec>::new(storage_config.clone())
                    .unwrap();
            let header = MockBlockHeader::default();
            let (prover_storage, ledger_state) = storage_manager.create_state_for(&header).unwrap();
            for test in tests.clone() {
                {
                    let mut working_set: WorkingSet<TestSpec> =
                        WorkingSet::new(prover_storage.clone());

                    working_set.set(&test.key, test.value.clone());
                    let (cache, _, witness) = working_set.checkpoint().0.freeze();
                    prover_storage
                        .validate_and_commit(cache, &witness)
                        .expect("storage is valid");
                    assert_eq!(
                        test.value,
                        prover_storage
                            .get::<User>(&test.key, None, &witness)
                            .unwrap()
                    );
                }
            }
            storage_manager
                .save_change_set(&header, prover_storage.to_change_set(), ledger_state.into())
                .unwrap();
            storage_manager.finalize(&header).unwrap();
        }

        {
            let mut storage_manager =
                ProverStorageManager::<MockDaSpec, DefaultStorageSpec>::new(storage_config)
                    .unwrap();
            let header = MockBlockHeader::default();
            let (storage, _) = storage_manager.create_state_for(&header).unwrap();
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
        let storage_config = sov_state::config::Config {
            path: tempdir.path().to_path_buf(),
        };
        {
            let mut storage_manager =
                ProverStorageManager::<MockDaSpec, DefaultStorageSpec>::new(storage_config.clone())
                    .unwrap();
            let header = MockBlockHeader::default();
            let (prover_storage, _) = storage_manager.create_state_for(&header).unwrap();
            assert!(prover_storage.is_empty());
        }

        let key = SlotKey::from_slice(b"some_key");
        let value = SlotValue::from("some_value");
        // First restart
        {
            let mut storage_manager =
                ProverStorageManager::<MockDaSpec, DefaultStorageSpec>::new(storage_config.clone())
                    .unwrap();
            let header = MockBlockHeader::default();
            let (prover_storage, ledger_state) = storage_manager.create_state_for(&header).unwrap();
            assert!(prover_storage.is_empty());
            let mut storage: WorkingSet<TestSpec> = WorkingSet::new(prover_storage.clone());
            storage.set(&key, value.clone());
            let (cache, _, witness) = storage.checkpoint().0.freeze();
            prover_storage
                .validate_and_commit(cache, &witness)
                .expect("storage is valid");
            storage_manager
                .save_change_set(&header, prover_storage.to_change_set(), ledger_state.into())
                .unwrap();
            storage_manager.finalize(&header).unwrap();
        }

        // Correctly restart from disk
        {
            let mut storage_manager =
                ProverStorageManager::<MockDaSpec, DefaultStorageSpec>::new(storage_config.clone())
                    .unwrap();
            let mock_block_header = MockBlockHeader::from_height(100000);
            let (prover_storage, _ledger_state) = storage_manager
                .create_state_for(&mock_block_header)
                .unwrap();
            assert!(!prover_storage.is_empty());
            assert_eq!(
                value,
                prover_storage
                    .get::<User>(&key, None, &Default::default())
                    .unwrap()
            );
        }
    }
}
