use std::convert::Infallible;
use std::sync::Arc;

use sov_modules_api::capabilities::mocks::MockKernel;
use sov_modules_api::capabilities::RollupHeight;
use sov_modules_api::{ApiStateAccessor, StateCheckpoint, StateValue};
use sov_state::{BorshCodec, Prefix};
use sov_test_utils::storage::SimpleStorageManager;
use unwrap_infallible::UnwrapInfallible;

use crate::state_tests::*;

fn increase_value_and_commit(
    state_value: &mut StateValue<u32>,
    storage: ProverStorage<StorageSpec>,
    kernel: &mut MockKernel<S>,
    storage_manager: &mut SimpleStorageManager<StorageSpec>,
) -> ProverStorage<StorageSpec> {
    let mut state: StateCheckpoint<S> = StateCheckpoint::new(storage.clone(), kernel);

    // Setting value, starting from 0
    let value = match state_value.get(&mut state).unwrap_infallible() {
        None => 0,
        Some(past) => past + 1,
    };

    state_value.set(&value, &mut state).unwrap_infallible();

    commit_to_storage(state, storage, kernel, storage_manager)
}

/// Tests that the archival state is correctly retrieved from the DB and updates to the head state don't interfere
#[test]
fn archival_state_updates_correctly() -> Result<(), Infallible> {
    let mut storage_manager = SimpleStorageManager::<StorageSpec>::new();
    let mut kernel = MockKernel::default();
    let mut state_value = StateValue::with_codec(Prefix::new(vec![0]), BorshCodec);

    for current_height in 0..100 {
        let storage = storage_manager.create_storage();
        let state_checkpoint = StateCheckpoint::new(storage.clone(), &kernel);
        let api_accessor = ApiStateAccessor::new(&state_checkpoint, Arc::new(kernel.clone()));

        for past_height in 0..current_height {
            let mut archival_api_accessor = api_accessor
                .get_archival_state(RollupHeight::new(past_height))
                .unwrap();

            let value = state_value.get(&mut archival_api_accessor)?;

            assert_eq!(value, Some(past_height as u32));
        }

        let storage =
            increase_value_and_commit(&mut state_value, storage, &mut kernel, &mut storage_manager);
        let state_checkpoint = StateCheckpoint::new(storage, &kernel);
        let api_accessor = ApiStateAccessor::new(&state_checkpoint, Arc::new(kernel.clone()));

        for another_past_height in 0..=current_height {
            let mut archival_api_accessor = api_accessor
                .get_archival_state(RollupHeight::new(another_past_height))
                .unwrap();

            let value = state_value.get(&mut archival_api_accessor)?;

            assert_eq!(value, Some(another_past_height as u32));
        }
    }

    Ok(())
}
