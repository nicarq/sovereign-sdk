use sov_modules_api::capabilities::mocks::MockKernel;
use sov_modules_api::capabilities::Kernel as _;
use sov_modules_api::{Spec, StateCheckpoint, StateReader, StateWriter, WorkingSet};
use sov_state::codec::BcsCodec;
use sov_state::namespaces::User;
use sov_state::{Kernel, SlotKey, SlotValue};
use sov_test_utils::storage::new_finalized_storage;
use sov_test_utils::TestSpec;

#[test]
fn test_workingset_get() {
    let tempdir = tempfile::tempdir().unwrap();
    let codec = BcsCodec {};
    let storage = new_finalized_storage(tempdir.path());

    let prefix = sov_state::Prefix::new(vec![1, 2, 3]);
    let storage_key = SlotKey::new(&prefix, &vec![4, 5, 6], &codec);
    let storage_value = SlotValue::new(&vec![7, 8, 9], &codec);

    let mut working_set =
        WorkingSet::<TestSpec>::new_deprecated(storage.clone(), &MockKernel::<TestSpec>::default());
    StateWriter::<User>::set(&mut working_set, &storage_key, storage_value.clone()).expect("The set operation should succeed because there should be enough funds in the metered working set");
    let value = StateReader::<User>::get(&mut working_set, &storage_key).expect("The get operation should succeed because there should be enough funds in the metered working set");

    assert_eq!(Some(storage_value), value);
}

#[test]
fn test_kernel_workingset_get() {
    let tempdir = tempfile::tempdir().unwrap();
    let codec = BcsCodec {};
    let storage = new_finalized_storage(tempdir.path());

    let prefix = sov_state::Prefix::new(vec![1, 2, 3]);
    let storage_key = SlotKey::new(&prefix, &vec![4, 5, 6], &codec);
    let storage_value = SlotValue::new(&vec![7, 8, 9], &codec);
    let kernel: MockKernel<TestSpec> = MockKernel::new(4, 1);

    let mut working_set =
        StateCheckpoint::<<TestSpec as Spec>::Storage>::new(storage.clone(), &kernel);
    let mut working_set = kernel.accessor(&mut working_set);

    StateWriter::<Kernel>::set(&mut working_set, &storage_key, storage_value.clone())
        .expect("This should be unfaillible");

    assert_eq!(
        Some(storage_value),
        StateReader::<Kernel>::get(&mut working_set, &storage_key)
            .expect("This should be unfaillible")
    );
}
