//! Tests for the state API.
//! Note: these tests should be moved back inside the source folder the `sov-modules-api` crate
//! as it directly uses structs that should be hidden from the public API.

mod archival;
mod namespaces;
mod structs;

use sov_mock_da::MockDaSpec;
use sov_mock_zkvm::MockZkvm;
use sov_modules_api::capabilities::mocks::MockKernel;
use sov_modules_api::{execution_mode, CryptoSpec, Spec, StateCheckpoint, Storage};
use sov_state::ProverStorage;
use sov_test_utils::storage::SimpleStorageManager;
use sov_test_utils::TestSpec;

pub type S = TestSpec;
pub type Zk =
    sov_modules_api::default_spec::DefaultSpec<MockDaSpec, MockZkvm, MockZkvm, execution_mode::Zk>;
pub type TestHasher = <<S as Spec>::CryptoSpec as CryptoSpec>::Hasher;
pub type StorageSpec = sov_state::DefaultStorageSpec<TestHasher>;

pub fn commit_to_storage<S: Spec<Storage = ProverStorage<StorageSpec>>>(
    state: StateCheckpoint<S::Storage>,
    storage: ProverStorage<StorageSpec>,
    kernel: &mut MockKernel<S>,
    storage_manager: &mut SimpleStorageManager<StorageSpec>,
) -> ProverStorage<StorageSpec> {
    let (cache_log, _, witness) = state.freeze();

    let (_, change_set) = storage
        .validate_and_materialize(cache_log, &witness)
        .expect("Native JMT validation should succeed");
    storage_manager.commit(change_set);

    kernel.increase_heights();

    storage_manager.create_storage()
}
