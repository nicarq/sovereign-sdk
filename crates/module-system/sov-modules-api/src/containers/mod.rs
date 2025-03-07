mod versioned_value;

pub(crate) mod map;
pub(crate) mod value;
pub(crate) mod vec;

pub use map::{AccessoryStateMap, KernelStateMap, StateMap, StateMapError};
pub use value::{AccessoryStateValue, KernelStateValue, StateValue, StateValueError};
pub use vec::{AccessoryStateVec, KernelStateVec, StateVec};
pub use versioned_value::VersionedStateValue;

/// A borrowed state value which points to state variable that it came from
// We use this struct to extend the borrow checker to state variables as if `self.state_value` was a reference.
// rather than a clone of a value from the external `impl TxState` struct. This borrow is purely imaginary.
// For now, all of the data is still actually cloned. In a future iteration, we might stored an `Arc` to the original value.
// in state which would allow us to avoid cloning/deserializing on each "borrow".

#[derive(Debug)]
pub struct Borrowed<'a, T, U> {
    value: T,
    _reference: &'a U,
}

impl<'a, T, U> std::ops::Deref for Borrowed<'a, T, U> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<'a, T, U> Borrowed<'a, T, U> {
    /// Builds a borrowed state value
    pub(crate) fn new(value: T, _reference: &'a U) -> Self {
        Self { value, _reference }
    }
}

#[cfg(test)]
mod test {

    use sov_mock_da::MockDaSpec;
    use sov_mock_zkvm::MockZkvm;
    use sov_rollup_interface::common::{IntoSlotNumber, SlotNumber};
    use sov_state::namespaces::User;
    use sov_state::{DefaultStorageSpec, SlotKey, SlotValue, Storage};
    use sov_test_utils::storage::SimpleStorageManager;

    use crate::capabilities::mocks::MockKernel;
    use crate::execution_mode::Native;
    use crate::{CryptoSpec, StateWriter, WorkingSet};

    type StorageSpec = DefaultStorageSpec<TestHasher>;
    type TestSpec = crate::default_spec::DefaultSpec<MockDaSpec, MockZkvm, MockZkvm, Native>;
    type TestHasher = <<TestSpec as crate::Spec>::CryptoSpec as CryptoSpec>::Hasher;

    #[derive(Clone)]
    struct TestCase {
        key: SlotKey,
        value: SlotValue,
        version: SlotNumber,
    }

    fn create_tests() -> Vec<TestCase> {
        vec![
            TestCase {
                key: SlotKey::from_slice(b"key_0"),
                value: SlotValue::from("value_0"),
                version: 0.to_slot_number(),
            },
            TestCase {
                key: SlotKey::from_slice(b"key_1"),
                value: SlotValue::from("value_1"),
                version: 1.to_slot_number(),
            },
            TestCase {
                key: SlotKey::from_slice(b"key_2"),
                value: SlotValue::from("value_2"),
                version: 2.to_slot_number(),
            },
            TestCase {
                key: SlotKey::from_slice(b"key_1"),
                value: SlotValue::from("value_3"),
                version: 3.to_slot_number(),
            },
        ]
    }

    #[test]
    fn test_jmt_storage() -> anyhow::Result<()> {
        let mut storage_manager = SimpleStorageManager::<StorageSpec>::new();
        let tests = create_tests();
        {
            let mut kernel = MockKernel::<TestSpec>::default();
            for test in &tests {
                {
                    let storage = storage_manager.create_storage();
                    let mut working_set: WorkingSet<TestSpec> =
                        WorkingSet::new_with_kernel(storage.clone(), &kernel);
                    StateWriter::<User>::set(&mut working_set, &test.key, test.value.clone())?;
                    let (scratchpad, _gas_meter, _) = working_set.finalize();
                    let checkpoint = scratchpad.commit();
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
        let mut storage_manager = SimpleStorageManager::new();
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
                WorkingSet::new_with_kernel(storage.clone(), &MockKernel::<TestSpec>::default());
            StateWriter::<User>::set(&mut working_set, &key, value.clone())?;
            let (cache, _, witness) = working_set.finalize().0.commit().freeze();
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
