use std::marker::PhantomData;
use std::sync::Arc;

use rockbound::cache::delta_reader::DeltaReader;
use rockbound::SchemaBatch;
use sov_db::accessory_db::AccessoryDb;
use sov_db::historical_state::HistoricalStateReader;
use sov_db::ledger_db::LedgerDb;
use sov_db::state_db::StateDb;
pub use sov_db::storage_manager::{
    NativeChangeSet, NativeStorageManager, NomtChangeSet, NomtStorageManager,
};
use sov_mock_da::{MockBlockHeader, MockDaSpec};
use sov_rollup_interface::da::BlockHeaderTrait;
use sov_rollup_interface::storage::HierarchicalStorageManager;
use sov_state::nomt::prover_storage::NomtProverStorage;
use sov_state::{
    MerkleProofSpec, NativeStorage, ProverStorage, StateAccesses, Storage, StorageRoot,
};
use tempfile::TempDir;

use crate::TestStorageSpec;

/// Implementation of [`HierarchicalStorageManager`] that provides [`ProverStorage`]
/// and commits changes directly to the underlying database.
pub struct SimpleStorageManager<S: MerkleProofSpec> {
    state: Arc<rockbound::DB>,
    accessory: Arc<rockbound::DB>,
    phantom_mp_spec: PhantomData<S>,
    // Holds ownership of [`Tempdir`] so it is not removed prematurely
    _dir: TempDir,
    root: StorageRoot<S>,
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
            _dir: dir,
            root: <ProverStorage<S> as Storage>::PRE_GENESIS_ROOT,
        }
    }

    /// Do JMT genesis;
    pub fn genesis(&mut self) {
        if self.root != <ProverStorage<S> as Storage>::PRE_GENESIS_ROOT {
            panic!("Cannot call genesis on non empty storage");
        }
        let prover_storage = self.create_storage();
        let witness = S::Witness::default();
        let state_accesses_genesis = StateAccesses {
            user: Default::default(),
            kernel: Default::default(),
        };

        let (root, change_set) = prover_storage
            .compute_state_update(
                state_accesses_genesis,
                &witness,
                <ProverStorage<S> as Storage>::PRE_GENESIS_ROOT,
            )
            .expect("state update computation must succeed");

        let changes = prover_storage.materialize_changes(change_set);
        self.commit(changes);
        self.root = root;
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
    /// Initialize a new instance in the given path.
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

    /// Create the new [`DeltaReader`] which has visibility only on persisted changes.
    pub fn create_ledger_storage(&mut self) -> DeltaReader {
        DeltaReader::new(self.db.clone(), Vec::new())
    }

    /// Write changes directly to underlying db
    pub fn commit(&mut self, ledger_change_set: SchemaBatch) {
        self.db.write_schemas(&ledger_change_set).unwrap();
    }
}

/// Implementation of [`HierarchicalStorageManager`] that provides [`NomtProverStorage`]
/// and commits changes directly to the underlying database.
pub struct SimpleNomtStorageManager<S: MerkleProofSpec> {
    // Holds ownership of [`Tempdir`] so it is not removed prematurely
    _dir: TempDir,
    state: sov_db::state_db_nomt::StateDb<S::Hasher>,
    historical_state: Arc<rockbound::DB>,
    accessory: Arc<rockbound::DB>,
    root: StorageRoot<S>,
}

impl<S: MerkleProofSpec> SimpleNomtStorageManager<S> {
    /// Initialize a new instance of [`SimpleNomtStorageManager`] in a temporary directory.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state_db = sov_db::state_db_nomt::StateDb::new(dir.path())
            .expect("Failed to initialize StateDb for NOMT");
        let historical_state_rocksdb = HistoricalStateReader::get_rockbound_options()
            .default_setup_db_in_path(dir.path())
            .unwrap();
        let accessory_rocksdb = AccessoryDb::get_rockbound_options()
            .default_setup_db_in_path(dir.path())
            .unwrap();

        Self {
            _dir: dir,
            state: state_db,
            historical_state: Arc::new(historical_state_rocksdb),
            accessory: Arc::new(accessory_rocksdb),
            root: <NomtProverStorage<S> as Storage>::PRE_GENESIS_ROOT,
        }
    }

    /// Create a new [`NomtProverStorage`] that has a view only on data written to disc.
    pub fn create_storage(&self) -> NomtProverStorage<S> {
        let state_session = self
            .state
            .begin_session_from_committed()
            .expect("Failed to begin state session");
        let historical_state_reader = HistoricalStateReader::with_delta_reader(DeltaReader::new(
            self.historical_state.clone(),
            Vec::new(),
        ))
        .expect("Failed to create historical state reader");
        let accessory_db =
            AccessoryDb::with_reader(DeltaReader::new(self.accessory.clone(), Vec::new()))
                .expect("Failed to create accessory db");

        NomtProverStorage::new(state_session, historical_state_reader, accessory_db)
    }

    /// Commit [`NomtChangeSet`] to disk.
    pub fn commit(&mut self, stf_change_set: NomtChangeSet) {
        let NomtChangeSet {
            state,
            historical_state,
            accessory,
        } = stf_change_set;

        self.state.commit_change_set(state).unwrap();
        self.accessory.write_schemas(&accessory).unwrap();
        self.historical_state
            .write_schemas(&historical_state)
            .unwrap();
    }
}

impl<S: MerkleProofSpec> Default for SimpleNomtStorageManager<S> {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait that represents a storage manager that is not attached to a particular DA layer.
/// All changes are linear.
/// Commited data will be available in the following call to create_prover_storage.
pub trait ForklessStorageManager {
    #[allow(missing_docs)]
    type Storage: NativeStorage;
    #[allow(missing_docs)]
    fn current_root(&self) -> <Self::Storage as Storage>::Root;
    #[allow(missing_docs)]
    fn create_storage_with_root(&mut self) -> (Self::Storage, <Self::Storage as Storage>::Root) {
        (self.create_prover_storage(), self.current_root())
    }
    #[allow(missing_docs)]
    fn create_prover_storage(&mut self) -> Self::Storage;
    #[allow(missing_docs)]
    fn commit_state_update(
        &mut self,
        storage: Self::Storage,
        state_update: <Self::Storage as Storage>::StateUpdate,
        new_root: <Self::Storage as Storage>::Root,
    ) {
        let change_set = storage.materialize_changes(state_update);
        self.commit_change_set(change_set, new_root);
    }
    #[allow(missing_docs)]
    fn commit_change_set(
        &mut self,
        change_set: <Self::Storage as Storage>::ChangeSet,
        new_root: <Self::Storage as Storage>::Root,
    );
}

impl ForklessStorageManager for SimpleStorageManager<TestStorageSpec> {
    type Storage = ProverStorage<TestStorageSpec>;

    fn current_root(&self) -> <Self::Storage as Storage>::Root {
        self.root
    }

    fn create_prover_storage(&mut self) -> Self::Storage {
        self.create_storage()
    }

    fn commit_change_set(
        &mut self,
        change_set: <Self::Storage as Storage>::ChangeSet,
        new_root: <Self::Storage as Storage>::Root,
    ) {
        self.commit(change_set);
        self.root = new_root;
    }
}

impl ForklessStorageManager for SimpleNomtStorageManager<TestStorageSpec> {
    type Storage = NomtProverStorage<TestStorageSpec>;

    fn current_root(&self) -> <Self::Storage as Storage>::Root {
        self.root
    }

    fn create_prover_storage(&mut self) -> Self::Storage {
        self.create_storage()
    }

    fn commit_change_set(
        &mut self,
        change_set: <Self::Storage as Storage>::ChangeSet,
        new_root: <Self::Storage as Storage>::Root,
    ) {
        self.commit(change_set);
        self.root = new_root;
    }
}

/// Using [`HierarchicalStorageManager`] to mimic [`SimpleStorageManager`],
/// but instead of commiting all data on disk, it just appends it to the following block.
/// Emulates fork-less DA without finality.
pub struct NonCommitingStorageManager<
    H: HierarchicalStorageManager<MockDaSpec, StfState = S>,
    S: Storage,
> {
    _dir: TempDir,
    storage_manager: H,
    last_block: MockBlockHeader,
    root: S::Root,
}

impl<H, S> NonCommitingStorageManager<H, S>
where
    H: HierarchicalStorageManager<MockDaSpec, StfState = S>,
    S: NativeStorage,
{
    /// Create the new [`NonCommitingStorageManager`].
    /// Passing [`TempDir`] allows keeping the directory from deletion.
    pub fn new(dir: TempDir, storage_manager: H) -> Self {
        let initial_block_header = MockBlockHeader::from_height(0);
        Self {
            _dir: dir,
            storage_manager,
            last_block: initial_block_header,
            root: S::PRE_GENESIS_ROOT,
        }
    }
}

impl<H, S> ForklessStorageManager for NonCommitingStorageManager<H, S>
where
    H: HierarchicalStorageManager<MockDaSpec, StfState = S, StfChangeSet = S::ChangeSet>,
    <H as HierarchicalStorageManager<MockDaSpec>>::LedgerChangeSet: Default,
    S: NativeStorage,
{
    type Storage = S;

    fn current_root(&self) -> <Self::Storage as Storage>::Root {
        self.root.clone()
    }

    fn create_prover_storage(&mut self) -> Self::Storage {
        let (prover_storage, _) = self
            .storage_manager
            .create_state_for(&self.last_block)
            .expect("Failed to create storage");
        prover_storage
    }

    fn commit_change_set(
        &mut self,
        change_set: <Self::Storage as Storage>::ChangeSet,
        new_root: <Self::Storage as Storage>::Root,
    ) {
        // Here is the trick, we don't commit, but chain it to the last block
        self.root = new_root;
        self.storage_manager
            .save_change_set(&self.last_block, change_set, Default::default())
            .expect("Failed to save change set");
        self.last_block =
            MockBlockHeader::from_height(self.last_block.height().checked_add(1).unwrap());
    }
}
