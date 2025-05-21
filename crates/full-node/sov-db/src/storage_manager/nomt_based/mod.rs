//! Implementation of [`HierarchicalStorageManager`] based on NOMT

mod groups;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use rockbound::cache::delta_reader::DeltaReader;
use rockbound::SchemaBatch;
use sov_rollup_interface::da::{BlockHeaderTrait, DaSpec};
use sov_rollup_interface::reexports::digest;
use sov_rollup_interface::storage::HierarchicalStorageManager;

use crate::accessory_db::AccessoryDb;
use crate::state_db_nomt::{StateOverlay, StateSession};
use crate::storage_manager::nomt_based::groups::{DbGroup, SnapshotGroup};

#[allow(missing_docs)]
pub struct NomtStateChangeSet {
    pub user: nomt::FinishedSession,
    pub kernel: nomt::FinishedSession,
}

impl NomtStateChangeSet {
    pub(crate) fn into_state_overlay(self) -> StateOverlay {
        let NomtStateChangeSet { user, kernel } = self;
        StateOverlay {
            user: user.into_overlay(),
            kernel: kernel.into_overlay(),
        }
    }
}

#[allow(missing_docs)]
pub struct NomtChangeSet {
    pub state: NomtStateChangeSet,
    pub accessory: SchemaBatch,
}

#[cfg(test)]
fn generate_empty_finished_session() -> nomt::FinishedSession {
    let dir = tempfile::tempdir().unwrap();

    let mut opts = crate::state_db_nomt::sov_nomt_default_options();
    opts.path(dir.path());
    let nomt = nomt::Nomt::<nomt::hasher::BinaryHasher<sha2::Sha256>>::open(opts).unwrap();
    let params = nomt::SessionParams::default().witness_mode(nomt::WitnessMode::read_write());
    nomt.begin_session(params).finish(Vec::new()).unwrap()
}

#[cfg(test)]
impl Default for NomtChangeSet {
    fn default() -> Self {
        Self {
            state: NomtStateChangeSet {
                user: generate_empty_finished_session(),
                kernel: generate_empty_finished_session(),
            },
            accessory: Default::default(),
        }
    }
}

/// The only thing [`NomtStorageManager`] needs to know about the thing it builds.
pub trait InitializableNativeNomtStorage<H>: Sized + Send + Sync {
    #[allow(missing_docs)]
    fn new(state_db: StateSession<H>, accessory_db: AccessoryDb) -> Self;
}

/// Implementation of [`HierarchicalStorageManager`] based on NOMT.
pub struct NomtStorageManager<Da: DaSpec, H, S: InitializableNativeNomtStorage<H>> {
    // L1 forks representation
    // Chain: prev_block -> child_blocks
    chain_forks: HashMap<Da::SlotHash, Vec<Da::SlotHash>>,
    // Reverse: child_block -> parent
    blocks_to_parent: HashMap<Da::SlotHash, Da::SlotHash>,
    snapshots: HashMap<Da::SlotHash, SnapshotGroup>,
    db_group: DbGroup<H>,

    _phantom_s: PhantomData<S>,
}

impl<Da, H, S> NomtStorageManager<Da, H, S>
where
    Da: DaSpec,
    H: digest::Digest<OutputSize = digest::typenum::U32> + Send + Sync,
    S: InitializableNativeNomtStorage<H>,
{
    /// Create a new [` NomtStorageManager`].
    pub fn new(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let db_group = DbGroup::new(path.as_ref().to_path_buf())?;
        Ok(Self {
            chain_forks: Default::default(),
            blocks_to_parent: Default::default(),
            snapshots: Default::default(),
            db_group,
            _phantom_s: Default::default(),
        })
    }

    // build a storage up to the given block_hash (inclusive).
    fn create_state_up_to(&self, block_hash: Da::SlotHash) -> anyhow::Result<(S, DeltaReader)> {
        // Snapshots are in reversed order
        let mut rev_snapshots = Vec::new();

        let mut current_hash = block_hash;
        while let Some(snapshot) = self.snapshots.get(&current_hash) {
            rev_snapshots.push(snapshot);
            match self.blocks_to_parent.get(&current_hash) {
                None => {
                    break;
                }
                Some(parent_hash) => {
                    current_hash = parent_hash.clone();
                }
            }
        }

        self.db_group.create_storage(&rev_snapshots)
    }

    fn finalize_by_hash_pair(
        &mut self,
        prev_block_hash: Da::SlotHash,
        current_block_hash: Da::SlotHash,
    ) -> anyhow::Result<()> {
        tracing::trace!(
            %prev_block_hash,
            %current_block_hash,
            "Finalizing block by pair of hashes"
        );
        if let Some(grand_parent) = self.blocks_to_parent.remove(&prev_block_hash) {
            self.finalize_by_hash_pair(grand_parent, prev_block_hash.clone())?;
        }
        let snapshot = self.snapshots.remove(&current_block_hash).ok_or_else(|| {
            anyhow::anyhow!(
                "No changes has been previously saved for block header prev_hash={} next_hash={}",
                prev_block_hash,
                current_block_hash,
            )
        })?;

        self.db_group.commit(snapshot)?;

        self.blocks_to_parent.remove(&current_block_hash);

        // All siblings of the current snapshot
        let mut to_discard: Vec<_> = self
            .chain_forks
            .remove(&prev_block_hash)
            .expect("Inconsistent chain_forks")
            .into_iter()
            .filter(|bh| bh != &current_block_hash)
            .collect();

        while let Some(block_hash) = to_discard.pop() {
            let child_block_hashes = self.chain_forks.remove(&block_hash).unwrap_or_default();
            self.blocks_to_parent
                .remove(&block_hash)
                .expect("Chain map inconsistency in `blocks_to_parent`");
            if self.snapshots.remove(&block_hash).is_some() {
                tracing::trace!(%block_hash, "Discarding snapshot");
            }
            to_discard.extend(child_block_hashes);
        }

        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.snapshots.is_empty() && self.blocks_to_parent.is_empty() && self.chain_forks.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn snapshots_count(&self) -> usize {
        self.snapshots.len()
    }

    #[cfg(test)]
    pub(crate) fn blocks_to_parent_count(&self) -> usize {
        self.blocks_to_parent.len()
    }
}

impl<Da, H, S> HierarchicalStorageManager<Da> for NomtStorageManager<Da, H, S>
where
    Da: DaSpec,
    H: digest::Digest<OutputSize = digest::typenum::U32> + Send + Sync,
    S: InitializableNativeNomtStorage<H>,
{
    type StfState = S;
    type StfChangeSet = NomtChangeSet;
    type LedgerState = DeltaReader;
    type LedgerChangeSet = SchemaBatch;

    fn create_bootstrap_state(&mut self) -> anyhow::Result<(Self::StfState, Self::LedgerState)> {
        let (stf_storage, ledger_storage) = self.db_group.create_storage(&[])?;
        Ok((stf_storage, ledger_storage))
    }

    fn create_state_for(
        &mut self,
        block_header: &Da::BlockHeader,
    ) -> anyhow::Result<(Self::StfState, Self::LedgerState)> {
        tracing::trace!(block_header = %block_header.display(), "Requested native storage");
        let prev_hash = block_header.prev_hash();
        let current_hash = block_header.hash();
        if let std::collections::hash_map::Entry::Vacant(e) =
            self.blocks_to_parent.entry(current_hash.clone())
        {
            self.chain_forks
                .entry(prev_hash.clone())
                .or_default()
                .push(current_hash.clone());
            e.insert(prev_hash);
        }

        let state = self.create_state_up_to(block_header.prev_hash())?;

        Ok(state)
    }

    fn create_state_after(
        &mut self,
        block_header: &Da::BlockHeader,
    ) -> anyhow::Result<(Self::StfState, Self::LedgerState)> {
        if !self.snapshots.contains_key(&block_header.hash()) {
            anyhow::bail!("There is no snapshot available for the block {}. Use `create_bootstrap_storage` for getting storage from finalized data.", block_header.display())
        }
        self.create_state_up_to(block_header.hash())
    }

    fn save_change_set(
        &mut self,
        block_header: &Da::BlockHeader,
        stf_change_set: Self::StfChangeSet,
        ledger_change_set: Self::LedgerChangeSet,
    ) -> anyhow::Result<()> {
        tracing::trace!(block_header = %block_header.display(), "Saving changes");

        if !self.chain_forks.contains_key(&block_header.prev_hash()) {
            anyhow::bail!(
                "Attempt to save changeset for unknown block header {}",
                block_header.display(),
            );
        }

        let block_hash = block_header.hash();
        if self.snapshots.contains_key(&block_hash) {
            anyhow::bail!(
                "Attempt to save changes for the same block {} twice. Probably a bug.",
                block_header.display()
            )
        }

        let NomtChangeSet {
            state,
            accessory: accessory_change_set,
        } = stf_change_set;

        let state_overlay = state.into_state_overlay();

        let snapshot = SnapshotGroup {
            state: state_overlay,
            accessory: Arc::new(accessory_change_set),
            ledger: Arc::new(ledger_change_set),
        };

        self.snapshots.insert(block_hash, snapshot);

        Ok(())
    }

    fn finalize(&mut self, block_header: &Da::BlockHeader) -> anyhow::Result<()> {
        tracing::trace!(block_hash = %block_header.hash(), "Finalizing changes");
        self.finalize_by_hash_pair(block_header.prev_hash(), block_header.hash())
    }
}
