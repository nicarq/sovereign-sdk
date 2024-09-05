use std::convert::Infallible;

use sov_modules_api::capabilities::mocks::MockKernel;
use sov_modules_api::{ApiStateAccessor, StateCheckpoint, StateValue};
use sov_state::Prefix;
use sov_test_utils::storage::SimpleStorageManager;
use unwrap_infallible::UnwrapInfallible;

use crate::state_tests::*;

fn increase_value_and_commit(
    state_value: &StateValue<u32>,
    storage: ProverStorage<StorageSpec>,
    kernel: &mut MockKernel<S>,
    storage_manager: &mut SimpleStorageManager<StorageSpec>,
) -> ProverStorage<StorageSpec> {
    let mut state: StateCheckpoint<S> = StateCheckpoint::new(storage.clone(), kernel);

    let value = state_value.get(&mut state).unwrap_infallible().unwrap_or(0);

    state_value
        .set(&(value + 1), &mut state)
        .unwrap_infallible();

    commit_to_storage(state, storage, kernel, storage_manager)
}

/// Tests that the archival state is correctly retrieved from the DB and updates to the head state don't interfere
#[test]
fn archival_state_updates_correctly() -> Result<(), Infallible> {
    let tmpdir = tempfile::tempdir().unwrap();
    let mut storage_manager = SimpleStorageManager::<StorageSpec>::new(tmpdir.path());
    let mut storage = storage_manager.create_storage();
    let mut kernel = MockKernel::default();

    let state_value = StateValue::new(Prefix::new(vec![0]));

    for i in 1..100 {
        let api_accessor = ApiStateAccessor::<S>::new(storage.clone());

        for j in 1..(i - 1) {
            let mut archival_api_accessor = api_accessor.get_archival_at(j);

            let value = state_value.get(&mut archival_api_accessor)?;

            assert_eq!(value, Some(j as u32));
        }

        storage =
            increase_value_and_commit(&state_value, storage, &mut kernel, &mut storage_manager);

        for j in 1..i {
            let mut archival_api_accessor = api_accessor.get_archival_at(j);

            let value = state_value.get(&mut archival_api_accessor)?;

            assert_eq!(value, Some(j as u32));
        }
    }

    Ok(())
}
