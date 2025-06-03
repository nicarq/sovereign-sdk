use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use nomt::hasher::BinaryHasher;
use nomt::{Nomt, Options, SessionParams, WitnessMode};
use sov_rollup_interface::reexports::digest;

/// Contains all the most recent rollup data.
pub struct NomtStateDb<H> {
    user: Nomt<BinaryHasher<H>>,
    kernel: Nomt<BinaryHasher<H>>,
}

impl<H: digest::Digest<OutputSize = digest::typenum::U32> + Send + Sync> NomtStateDb<H> {
    /// Initialize a new [` NomtStateDb `] in the given path.
    pub fn new(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let user = {
            let mut opts = sov_nomt_default_options();
            opts.path(path.as_ref().join("user_nomt_db"));
            Nomt::<BinaryHasher<H>>::open(opts)?
        };
        let kernel = {
            let mut opts = sov_nomt_default_options();
            opts.path(path.as_ref().join("kernel_nomt_db"));
            Nomt::<BinaryHasher<H>>::open(opts)?
        };

        Ok(Self { user, kernel })
    }

    /// Commit [`StateOverlay`] to disk.
    pub(crate) fn commit(&self, overlay: StateOverlay) -> anyhow::Result<()> {
        let StateOverlay { user, kernel } = overlay;
        // TODO: Fail of kernel commit will leave user data inconsistent.
        //   to be addressed in follow up: https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/2634
        user.commit(&self.user)?;
        kernel.commit(&self.kernel)?;
        Ok(())
    }

    /// Commit [`crate::storage_manager::StateFinishedSession`] to disk.
    #[cfg(feature = "test-utils")]
    pub fn commit_change_set(
        &self,
        session: crate::storage_manager::StateFinishedSession,
    ) -> anyhow::Result<()> {
        let overlay = session.into_state_overlay();
        self.commit(overlay)
    }
}

/// Combination of [`nomt::Overlay`] for user and kernel namespaces.
pub(crate) struct StateOverlay {
    pub(crate) user: nomt::Overlay,
    pub(crate) kernel: nomt::Overlay,
}

/// Container of all necessary information to build a [`nomt::Session`] for a given namespace.
pub struct NomtSessionBuilder<H, K> {
    state_db: Arc<NomtStateDb<H>>,
    // In revered chronological order, as [`nomt::SessionParams::overlay`] expects
    relevant_snapshot_refs: Vec<K>,
    // Reference to snapshots for all blocks currently available in a storage manager.
    // Used to get reference to state overlays and build session.
    all_snapshots: Arc<RwLock<HashMap<K, StateOverlay>>>,
}

impl<H, K: Clone> Clone for NomtSessionBuilder<H, K> {
    fn clone(&self) -> Self {
        Self {
            state_db: self.state_db.clone(),
            relevant_snapshot_refs: self.relevant_snapshot_refs.clone(),
            all_snapshots: self.all_snapshots.clone(),
        }
    }
}

impl<H, K> NomtSessionBuilder<H, K> {
    /// Parameters:
    ///  * `state_db` - Reference to [`NomtStateDb`].
    ///  * `relevant_snapshot_refs`: In revered chronological order, as [`nomt::SessionParams::overlay`] expects
    ///  * `all_snapshots`. Should be the same structure that is used by a storage manager
    #[allow(dead_code)]
    pub(crate) fn new(
        state_db: Arc<NomtStateDb<H>>,
        relevant_snapshot_refs: Vec<K>,
        all_snapshots: Arc<RwLock<HashMap<K, StateOverlay>>>,
    ) -> Self {
        Self {
            state_db,
            relevant_snapshot_refs,
            all_snapshots,
        }
    }
}

impl<H, K> NomtSessionBuilder<H, K>
where
    K: Eq + std::hash::Hash,
    H: digest::Digest<OutputSize = digest::typenum::U32> + Send + Sync,
{
    /// Build [`nomt::Session`] for [`crate::namespaces::UserNamespace`].
    /// Should be called only when really needed.
    /// Will hold the read lock to all snapshots.
    /// **Commiting storage will be blocked until all built sessions are deallocated.**
    #[tracing::instrument(skip(self))]
    pub fn begin_user_session(&self) -> anyhow::Result<nomt::Session<BinaryHasher<H>>> {
        let params = {
            let mut overlays = Vec::with_capacity(self.relevant_snapshot_refs.len());
            let snapshots = self.all_snapshots.read().expect("Snapshots lock poisoned");
            for overlay_ref in &self.relevant_snapshot_refs {
                let Some(state_overlay) = snapshots.get(overlay_ref) else {
                    tracing::debug!(
                        "Cannot find snapshot from reference, assuming it has been committed"
                    );
                    continue;
                };
                overlays.push(&state_overlay.user);
            }
            SessionParams::default()
                .overlay(overlays)
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to construct session params for user session: {:?}",
                        e
                    )
                })?
                .witness_mode(WitnessMode::read_write())
        };
        Ok(self.state_db.user.begin_session(params))
    }

    /// Build [`nomt::Session`] for [`crate::namespaces::KernelNamespace`].
    /// Should be called only when really needed.
    /// Will hold the read lock to all snapshots.
    /// **Commiting storage will be blocked until all built sessions are deallocated.**
    #[tracing::instrument(skip(self))]
    pub fn begin_kernel_session(&self) -> anyhow::Result<nomt::Session<BinaryHasher<H>>> {
        let params = {
            let mut overlays = Vec::with_capacity(self.relevant_snapshot_refs.len());
            let snapshots = self.all_snapshots.read().expect("Snapshots lock poisoned");
            for overlay_ref in &self.relevant_snapshot_refs {
                let Some(state_overlay) = snapshots.get(overlay_ref) else {
                    tracing::debug!(
                        "Cannot find snapshot from reference, assuming it has been committed"
                    );
                    continue;
                };
                overlays.push(&state_overlay.kernel);
            }
            SessionParams::default()
                .overlay(overlays)
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to construct session params for kernel session: {:?}",
                        e
                    )
                })?
                .witness_mode(WitnessMode::read_write())
        };
        Ok(self.state_db.kernel.begin_session(params))
    }
}

/// Begin a new user and kernel session with only data that has been written to disk
#[cfg(feature = "test-utils")]
pub fn get_session_builder_from_committed<H, K>(
    state_db: Arc<NomtStateDb<H>>,
) -> NomtSessionBuilder<H, K>
where
    K: Clone + Eq + std::hash::Hash,
    H: digest::Digest<OutputSize = digest::typenum::U32> + Send + Sync,
{
    let empty_snapshots = Arc::new(RwLock::new(HashMap::new()));
    NomtSessionBuilder::new(state_db, Vec::new(), empty_snapshots)
}

/// All non-path-related options, tuned for optimal performance in sov-rollup
pub(crate) fn sov_nomt_default_options() -> Options {
    let mut opts = Options::new();
    // Draft values, needs to be benchmarked on the target system type.
    opts.commit_concurrency(2);
    opts.prepopulate_page_cache(true);
    opts
}

#[cfg(test)]
mod tests {
    use sha2::Digest;

    use super::*;
    use crate::storage_manager::StateFinishedSession;
    use crate::test_utils::H;

    fn from_key(key: u64) -> (nomt::trie::KeyPath, Option<Vec<u8>>) {
        let raw_data = key.to_be_bytes();
        let key_path: nomt::trie::KeyPath = H::digest(raw_data).into();
        (key_path, Some(raw_data.to_vec()))
    }

    #[test]
    fn test_session_can_be_built_while_finalized() {
        let temp_dir = tempfile::tempdir().unwrap();
        let state_db = Arc::new(NomtStateDb::<H>::new(temp_dir.path()).unwrap());

        // First produce some overlays with data
        let all_overlays: HashMap<u64, StateOverlay> = HashMap::new();
        let all_overlays = Arc::new(RwLock::new(all_overlays));
        let rounds = 10;
        for this_ref in 0..rounds {
            let mut overlay_refs = (0..this_ref).collect::<Vec<_>>();
            overlay_refs.reverse();
            let builder = NomtSessionBuilder::<H, u64>::new(
                state_db.clone(),
                overlay_refs,
                all_overlays.clone(),
            );

            let (key_path, data) = from_key(this_ref);
            let writes = vec![(key_path, nomt::KeyReadWrite::Write(data))];

            let user_session = builder.begin_user_session().unwrap();
            let finished_user_session = user_session.finish(writes.clone()).unwrap();

            let kernel_session = builder.begin_kernel_session().unwrap();

            let finished_kernel_session = kernel_session.finish(writes.clone()).unwrap();
            let overlay = StateFinishedSession::new(finished_user_session, finished_kernel_session)
                .into_state_overlay();
            let mut overlays = all_overlays.write().unwrap();
            overlays.insert(this_ref, overlay);
        }
        let mut overlay_refs = (0..rounds).collect::<Vec<_>>();
        overlay_refs.reverse();
        let check_builder =
            NomtSessionBuilder::<H, u64>::new(state_db.clone(), overlay_refs, all_overlays.clone());
        for commiting_ref in 0..rounds {
            let user_session = check_builder.begin_user_session().unwrap();
            let kernel_session = check_builder.begin_kernel_session().unwrap();
            for this_ref in 0..rounds {
                let (key_path, expected_value) = from_key(this_ref);
                let user_value = user_session.read(key_path).unwrap();
                let kernel_value = kernel_session.read(key_path).unwrap();

                assert_eq!(
                    user_value, expected_value,
                    "failed to check value for ref: {}",
                    this_ref
                );
                assert_eq!(kernel_value, expected_value);
            }
            drop(user_session);
            drop(kernel_session);
            let mut overlays = all_overlays.write().unwrap();
            let overlay = overlays.remove(&commiting_ref).unwrap();
            state_db.commit(overlay).unwrap();
        }
    }
}
