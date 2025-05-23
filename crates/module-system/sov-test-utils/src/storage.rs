use std::marker::PhantomData;
use std::sync::Arc;

use rockbound::cache::delta_reader::DeltaReader;
use rockbound::SchemaBatch;
use sov_db::accessory_db::AccessoryDb;
use sov_db::ledger_db::LedgerDb;
use sov_db::state_db::StateDb;
pub use sov_db::storage_manager::{NativeChangeSet, NativeStorageManager};
use sov_state::{MerkleProofSpec, ProverStorage};

/// Implementation of [`sov_rollup_interface::storage::HierarchicalStorageManager`] that provides [`ProverStorage`]
/// and commits changes directly to the underlying database.
pub struct SimpleStorageManager<S: MerkleProofSpec> {
    state: Arc<rockbound::DB>,
    accessory: Arc<rockbound::DB>,
    phantom_mp_spec: PhantomData<S>,
    // Holds ownership of [`Tempdir`] so it is not removed prematurely
    #[allow(dead_code)]
    dir: tempfile::TempDir,
}

impl<S: MerkleProofSpec> Default for SimpleStorageManager<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: MerkleProofSpec> SimpleStorageManager<S> {
    /// Initialize new instance in temporary folder.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state_rocksdb = StateDb::get_rockbound_options()
            .default_setup_db_in_path(dir.path())
            .unwrap();
        let accessory_rocksdb = AccessoryDb::get_rockbound_options()
            .default_setup_db_in_path(dir.path())
            .unwrap();
        Self {
            state: Arc::new(state_rocksdb),
            accessory: Arc::new(accessory_rocksdb),
            phantom_mp_spec: Default::default(),
            dir,
        }
    }

    /// Create a new [` ProverStorage `] that has a view only on data written to disc.
    pub fn create_storage(&self) -> ProverStorage<S> {
        let state_reader = DeltaReader::new(self.state.clone(), Vec::new());
        let state_db = StateDb::with_delta_reader(state_reader).unwrap();

        let accessory_reader = DeltaReader::new(self.accessory.clone(), Vec::new());
        let accessory_db = AccessoryDb::with_reader(accessory_reader).unwrap();
        ProverStorage::with_db_handles(state_db, accessory_db)
    }

    /// Saves changes directly to disk.
    // If we want it faster, can keep in memory
    pub fn commit(&mut self, stf_change_set: NativeChangeSet) {
        let NativeChangeSet {
            state_change_set,
            accessory_change_set,
        } = stf_change_set;
        self.state.write_schemas(&state_change_set).unwrap();
        self.accessory.write_schemas(&accessory_change_set).unwrap();
    }
}

/// Storage manager suitable for [`LedgerDb`].
pub struct SimpleLedgerStorageManager {
    db: Arc<rockbound::DB>,
}

impl SimpleLedgerStorageManager {
    /// Initialize new instance in the given path.
    pub fn new(path: impl AsRef<std::path::Path>) -> Self {
        let db = LedgerDb::get_rockbound_options()
            .default_setup_db_in_path(path.as_ref())
            .unwrap();
        Self { db: Arc::new(db) }
    }

    /// Initialize new instance at unspecified path.
    pub fn new_any_path() -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self::new(dir.path())
    }

    /// Create new [`DeltaReader`] which has visibility only on persisted changes.
    pub fn create_ledger_storage(&mut self) -> DeltaReader {
        DeltaReader::new(self.db.clone(), Vec::new())
    }

    /// Write changes directly to underlying db
    pub fn commit(&mut self, ledger_change_set: SchemaBatch) {
        self.db.write_schemas(&ledger_change_set).unwrap();
    }
}
