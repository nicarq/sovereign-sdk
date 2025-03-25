#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

pub(crate) mod common;
mod config;
mod rest_api;
mod tx_status;

pub mod preferred;
pub mod standard;
#[cfg(feature = "test-utils")]
pub mod test_stateless;

use std::sync::Arc;

use axum::async_trait;
pub use common::Sequencer;
pub use config::{SequencerConfig, SequencerKindConfig};
pub use rest_api::SequencerApis;
use serde::Serialize;
use sov_modules_api::{RuntimeEventProcessor, RuntimeEventResponse};
use sov_rollup_interface::TxHash;
pub use tx_status::TxStatusManager;

pub use crate::tx_status::TxStatus;

/// The response type to REST API calls that successfully publish a batch.
#[derive(Debug, Clone, Serialize)]
pub struct SubmitBatchReceipt {
    /// All the hashes of the transactions that were successfully included in
    /// the batch.
    pub tx_hashes: Arc<[TxHash]>,
}

/// See [`crate::common::Sequencer::is_ready`].
#[derive(Debug, serde::Serialize)]
pub enum SequencerNotReadyDetails {
    /// The node is catching up to the chain tip
    Syncing {
        #[allow(missing_docs)]
        target_da_height: u64,
        #[allow(missing_docs)]
        synced_da_height: u64,
    },
    /// The sequencer is waiting for the DA to finalize more blocks
    WaitingOnDa {
        #[allow(missing_docs)]
        finalized_da_height: u64,
        #[allow(missing_docs)]
        needed_finalized_height: u64,
    },
}

/// See [`crate::common::Sequencer::subscribe_events`].
#[derive(derivative::Derivative, serde::Serialize, serde::Deserialize)]
#[derivative(Clone(bound = ""))]
#[serde(bound = "")]
pub struct SequencerEvent<Rt: RuntimeEventProcessor> {
    /// The hash of the transaction for which the event was emitted.
    pub tx_hash: TxHash,
    /// Event data.
    #[serde(flatten)]
    pub event: RuntimeEventResponse<Rt::RuntimeEvent>,
}

/// An object-safe interface to the preferred sequencer, which can be used to
/// get a sequence number assigned to preferred proof blobs.
#[async_trait]
pub trait SequenceNumberProvider: Send + Sync + 'static {
    /// Generates the next sequence number to use for a new preferred proof blob.
    ///
    /// Subsequent calls to this method MUST return different (greater) values.
    async fn generate_sequence_number(&self, preferred_blob: &[u8]) -> anyhow::Result<u64>;
}
