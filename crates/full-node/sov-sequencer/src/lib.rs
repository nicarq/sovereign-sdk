#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

mod blob_sender;
pub(crate) mod common;
mod config;
mod rest_api;
mod tx_status;

pub mod preferred;
pub mod standard;
#[cfg(feature = "test-utils")]
pub mod test_stateless;

use axum::async_trait;
pub use common::Sequencer;
pub use config::{SequencerConfig, SequencerKindConfig};
pub use rest_api::SequencerApis;
use serde::Serialize;
use sov_modules_api::{DaSpec, RuntimeEventProcessor, RuntimeEventResponse};
use sov_rollup_interface::node::da::{DaService, SubmitBlobReceipt};
use sov_rollup_interface::TxHash;
use tokio::sync::oneshot;
pub use tx_status::TxStatusManager;

pub use crate::tx_status::TxStatus;

/// The response type to REST API calls that successfully publish a batch.
#[derive(Debug, Clone, Serialize)]
pub struct SubmitBatchReceipt<Da: DaSpec> {
    /// All the hashes of the transactions that were successfully included in
    /// the batch.
    pub tx_hashes: Vec<TxHash>,
    /// Blob metadata to track its status.
    #[serde(flatten)]
    pub submit_blob_receipt: SubmitBlobReceipt<Da::TransactionId>,
}

pub(crate) type BlobReceiptFut<Da> = oneshot::Receiver<
    Result<
        SubmitBlobReceipt<<<Da as DaService>::Spec as DaSpec>::TransactionId>,
        <Da as DaService>::Error,
    >,
>;

/// See [`crate::common::Sequencer::is_ready`].
#[derive(Debug, serde::Serialize)]
pub struct SequencerNotReadyDetails {
    #[allow(missing_docs)]
    pub target_da_height: u64,
    #[allow(missing_docs)]
    pub synced_da_height: u64,
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
