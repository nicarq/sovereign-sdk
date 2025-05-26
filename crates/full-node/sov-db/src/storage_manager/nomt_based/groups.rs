use std::sync::Arc;

use rockbound::cache::delta_reader::DeltaReader;
use rockbound::SchemaBatch;
use sov_rollup_interface::reexports::digest;

use crate::accessory_db::AccessoryDb;
use crate::ledger_db::LedgerDb;
use crate::state_db_nomt::{StateDb, StateOverlay};
use crate::storage_manager::{update_ledger_finalized_height, InitializableNativeNomtStorage};

pub(crate) struct DbGroup<H> {
    state: StateDb<H>,
    accessory: Arc<rockbound::DB>,
    ledger: Arc<rockbound::DB>,
}

impl<H: digest::Digest<OutputSize = digest::typenum::U32> + Send + Sync> DbGroup<H> {
    pub(crate) fn new(path: std::path::PathBuf) -> anyhow::Result<Self> {
        let state_db = StateDb::<H>::new(&path)?;
        let accessory_rocksdb =
            AccessoryDb::get_rockbound_options().default_setup_db_in_path(&path)?;
        let ledger_rocksdb = LedgerDb::get_rockbound_options().default_setup_db_in_path(&path)?;
        Ok(Self {
            state: state_db,
            accessory: Arc::new(accessory_rocksdb),
            ledger: Arc::new(ledger_rocksdb),
        })
    }

    pub(crate) fn commit(&mut self, snapshot: SnapshotGroup) -> anyhow::Result<()> {
        let SnapshotGroup {
            state,
            accessory,
            ledger,
        } = snapshot;

        self.state.commit(state)?;
        self.accessory.write_schemas(&accessory)?;
        // Ledger goes last, as its data is used during the start.
        // So if ledger save failed, state and accessory will be synced from DA
        self.ledger.write_schemas(&ledger)?;
        Ok(())
    }

    pub(crate) fn create_storage<S: InitializableNativeNomtStorage<H>>(
        &self,
        rev_snapshots: &[&SnapshotGroup],
    ) -> anyhow::Result<(S, DeltaReader)> {
        let mut accessory_snapshots = Vec::with_capacity(rev_snapshots.len());
        let mut ledger_snapshots = Vec::with_capacity(rev_snapshots.len());
        let mut state_overlays = Vec::with_capacity(rev_snapshots.len());

        for snapshot in rev_snapshots {
            accessory_snapshots.push(snapshot.accessory.clone());
            ledger_snapshots.push(snapshot.ledger.clone());
            state_overlays.push(&snapshot.state);
        }

        let state_session = self.state.begin_session(state_overlays)?;

        let accessory_reader = DeltaReader::new(self.accessory.clone(), accessory_snapshots);
        let accessory_db = AccessoryDb::with_reader(accessory_reader)?;
        let ledger_reader = DeltaReader::new(self.ledger.clone(), ledger_snapshots);

        let storage = S::new(state_session, accessory_db);
        Ok((storage, ledger_reader))
    }

    pub(crate) fn update_ledger_finalized_height(&self) -> anyhow::Result<()> {
        update_ledger_finalized_height(self.ledger.clone())
    }
}

pub(crate) struct SnapshotGroup {
    pub(crate) state: StateOverlay,
    pub(crate) accessory: Arc<SchemaBatch>,
    pub(crate) ledger: Arc<SchemaBatch>,
}
