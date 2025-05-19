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
use sov_test_utils::{validate_and_materialize, TestSpec};

pub type S = TestSpec;
pub type Zk =
    sov_modules_api::default_spec::DefaultSpec<MockDaSpec, MockZkvm, MockZkvm, execution_mode::Zk>;
pub type TestHasher = <<S as Spec>::CryptoSpec as CryptoSpec>::Hasher;
pub type StorageSpec = sov_state::DefaultStorageSpec<TestHasher>;

pub fn commit_to_storage<S: Spec<Storage = ProverStorage<StorageSpec>>>(
    state: StateCheckpoint<S>,
    storage: ProverStorage<StorageSpec>,
    kernel: &mut MockKernel<S>,
    storage_manager: &mut SimpleStorageManager<StorageSpec>,
    pre_state_root: <<S as Spec>::Storage as Storage>::Root,
) -> (
    ProverStorage<StorageSpec>,
    <<S as Spec>::Storage as Storage>::Root,
) {
    let (cache_log, _, witness) = state.freeze();

    let (root, change_set) = validate_and_materialize(storage, cache_log, &witness, pre_state_root)
        .expect("Native JMT validation should succeed");
    storage_manager.commit(change_set);

    kernel.increase_heights();

    (storage_manager.create_storage(), root)
}
