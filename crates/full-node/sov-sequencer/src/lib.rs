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
#[allow(missing_docs)]
pub enum SequencerNotReadyDetails {
    /// The node is catching up to the chain tip
    Syncing {
        target_da_height: u64,
        synced_da_height: u64,
    },
    /// The sequencer is waiting for the DA to finalize more blocks
    WaitingOnDa {
        finalized_da_height: u64,
        needed_finalized_height: u64,
    },
    /// The sequencer is waiting for the node to catch up to them
    WaitingOnNode {
        current_node_height: u64,
        current_sequencer_height: u64,
        current_delta: u64,
        max_allowed_delta: u64,
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

/// An object-safe interface to the sequencer, which can be used to
/// publish a proof blob to DA.
#[async_trait]
pub trait ProofBlobSender: Send + Sync + 'static {
    /// Publishes a proof blob to DA.
    async fn publish_proof_blob(&self, proof_blob: Arc<[u8]>) -> anyhow::Result<()>;
}
