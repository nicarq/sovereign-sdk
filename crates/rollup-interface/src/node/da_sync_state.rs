use std::sync::atomic::AtomicU64;

use super::da::DaService;
use crate::da::BlockHeaderTrait;

/// The state necessary to track the sync status of the node
#[derive(Debug, Default)]
pub struct DaSyncState {
    /// Last processed DA height.
    pub synced_da_height: AtomicU64,
    /// Latest known DA height.
    pub target_da_height: AtomicU64,
}

/// The status of the current sync
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyncStatus {
    /// The node has caught up to the chain tip
    Synced {
        /// The current height through which we've synced
        synced_da_height: u64,
    },
    /// The node is currently syncing
    Syncing {
        /// The current height through which we've synced
        synced_da_height: u64,
        /// The height to which we're syncing. This reflects the current view of the DA chain tip
        target_da_height: u64,
    },
}

impl SyncStatus {
    /// Returns true if the sync status is `Synced`
    pub fn is_synced(&self) -> bool {
        match self {
            SyncStatus::Synced { .. } => true,
            SyncStatus::Syncing { .. } => false,
        }
    }

    /// Distance to the chain head.
    pub fn distance(&self) -> u64 {
        match self {
            SyncStatus::Synced { .. } => 0,
            SyncStatus::Syncing {
                synced_da_height,
                target_da_height,
            } => target_da_height.saturating_sub(*synced_da_height),
        }
    }
}

impl DaSyncState {
    /// Updates the target height of the sync state using the provided
    /// [`DaService`].
    pub async fn update_target<Da: DaService<Error = anyhow::Error>>(
        &self,
        da_service: &Da,
    ) -> anyhow::Result<()> {
        let target_da_height = da_service.get_head_block_header().await?.height();
        self.target_da_height
            .store(target_da_height, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    /// Latest known sync status.
    pub fn status(&self) -> SyncStatus {
        let current = self
            .synced_da_height
            .load(std::sync::atomic::Ordering::Acquire);
        let target = self
            .target_da_height
            .load(std::sync::atomic::Ordering::Acquire);

        if current == target {
            SyncStatus::Synced {
                synced_da_height: current,
            }
        } else {
            SyncStatus::Syncing {
                synced_da_height: current,
                target_da_height: target,
            }
        }
    }
}
