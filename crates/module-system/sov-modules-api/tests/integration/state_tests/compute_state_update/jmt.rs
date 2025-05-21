// This is a reference point to validate that there are no errors in test

use sov_state::{ArrayWitness, ProverStorage, StateAccesses, Storage, StorageRoot, ZkStorage};
use sov_test_utils::storage::SimpleStorageManager;

use crate::state_tests::compute_state_update::{run_test, ProverStorageLifeCycle, TestCase};
use crate::state_tests::StorageSpec;

struct SimpleStorageManagerWithRoot {
    storage_manager: SimpleStorageManager<StorageSpec>,
    root: StorageRoot<StorageSpec>,
}

impl SimpleStorageManagerWithRoot {
    pub fn new() -> Self {
        let mut storage_manager = SimpleStorageManager::new();
        let root = genesis_prover_storage(&mut storage_manager);
        Self {
            storage_manager,
            root,
        }
    }
}

impl ProverStorageLifeCycle for SimpleStorageManagerWithRoot {
    type Storage = ProverStorage<StorageSpec>;

    fn create_new(&mut self) -> (Self::Storage, <Self::Storage as Storage>::Root) {
        (self.storage_manager.create_storage(), self.root)
    }

    fn save_and_commit(
        &mut self,
        storage: Self::Storage,
        state_update: <Self::Storage as Storage>::StateUpdate,
        new_root: <Self::Storage as Storage>::Root,
    ) {
        let changes = storage.materialize_changes(state_update);
        self.storage_manager.commit(changes);
        self.root = new_root;
    }
}

fn genesis_prover_storage(sm: &mut SimpleStorageManager<StorageSpec>) -> StorageRoot<StorageSpec> {
    {
        let prover_storage = sm.create_storage();
        let witness = ArrayWitness::default();
        let state_accesses_genesis = StateAccesses {
            user: Default::default(),
            kernel: Default::default(),
        };

        let (root, change_set) = prover_storage
            .compute_state_update(
                state_accesses_genesis,
                &witness,
                <ProverStorage<StorageSpec> as Storage>::PRE_GENESIS_ROOT,
            )
            .expect("state update computation must succeed");

        let changes = prover_storage.materialize_changes(change_set);
        sm.commit(changes);
        root
    }
}

fn run_jmt_test(test_case: TestCase) {
    let sm = SimpleStorageManagerWithRoot::new();
    run_test(test_case, sm, ZkStorage::<StorageSpec>::new());
}

#[test]
fn test_roundtrip_jmt() {
    run_jmt_test(TestCase::single_write());
    run_jmt_test(TestCase::single_write_both_namespaces());
    run_jmt_test(TestCase::single_read_write_different_key());
    run_jmt_test(TestCase::single_read_write_same_key());
    run_jmt_test(TestCase::rounds_of_same_key());
}
