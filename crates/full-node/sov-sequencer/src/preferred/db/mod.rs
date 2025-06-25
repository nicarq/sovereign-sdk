//! Database for sequencer-related data.
//!
//! TODO(@neysofu): Remove *all* blocking code inside async functions.
//!
//! # About [`assert!`]
//!
//! Preferred sequencer logic is hard to reason about, hard to get right, and
//! most importantly business-critical. We strive to be intentional about
//! invariants and we'd rather have an application crash due to broken
//! invariants than to have bugs that result in subtle state inconsistencies.

pub mod postgres;
pub mod rocksdb;

use std::collections::VecDeque;
use std::marker::PhantomData;
use std::num::NonZero;
use std::sync::Arc;

use axum::async_trait;
use borsh::{BorshDeserialize, BorshSerialize};
use sov_blob_sender::{new_blob_id, BlobInternalId};
use sov_blob_storage::{PreferredBatchData, SequenceNumber};
use sov_modules_api::capabilities::BlobSelector;
use sov_modules_api::{
    FullyBakedTx, KernelStateAccessor, Runtime, Spec, StateCheckpoint, StateUpdateInfo, TxHash,
    VisibleSlotNumber,
};
use tokio::sync::mpsc;
use tracing::info;
#[cfg(test)]
mod tests;

use crate::common::WithCachedTxHashes;
use crate::metrics::track_sequence_number;

#[async_trait]
pub trait PreferredSequencerDbBackend: Send + Sync + 'static {
    async fn begin_rollup_block(
        &mut self,
        sequence_number: SequenceNumber,
        blob_id: BlobInternalId,
        visible_slot_number_after_increase: VisibleSlotNumber,
        visibile_slots_to_advance: NonZero<u8>,
    ) -> anyhow::Result<()>;

    /// Calls to this method MUST be "sandwiched" between
    /// [`PreferredSequencerDbBackend::begin_rollup_block`] and
    /// [`PreferredSequencerDbBackend::end_rollup_block`].
    async fn add_tx(
        &mut self,
        sequence_number_of_in_progress_batch: SequenceNumber,
        tx_idx_within_batch: u64,
        tx: FullyBakedTx,
        hash: TxHash,
    ) -> anyhow::Result<()>;

    async fn end_rollup_block(
        &mut self,
        cached: &PreferredSequencerReadBatch,
    ) -> anyhow::Result<()>;

    async fn read_completed_blobs(&self) -> anyhow::Result<Vec<PreferredSequencerReadBlob>>;

    async fn read_in_progress_batch(&self) -> anyhow::Result<Option<PreferredSequencerReadBatch>>;

    async fn add_proof_blob(
        &mut self,
        sequence_number: SequenceNumber,
        blob_id: BlobInternalId,
        data: Arc<[u8]>,
    ) -> anyhow::Result<()>;

    /// Instructs the database it MAY delete all data up to the given
    /// [`SequenceNumber`] (included).
    ///
    /// This method exists because the sequencer has no use for data that is
    /// already finalized.
    async fn prune(&mut self, up_to_including: SequenceNumber) -> anyhow::Result<()>;
}

/// See [`PreferredSequencerReadBlob::Batch`].
#[derive(Debug, Clone)]
pub struct PreferredSequencerReadBatch {
    pub sequence_number: SequenceNumber,
    pub visible_slot_number_after_increase: VisibleSlotNumber,
    pub visible_slots_to_advance: NonZero<u8>,
    pub blob_id: BlobInternalId,
    pub txs: Vec<FullyBakedTx>,
    pub tx_hashes: Vec<TxHash>,
}

impl PreferredSequencerReadBatch {
    pub(crate) fn into_with_cached_tx_hashes(self) -> WithCachedTxHashes<PreferredBatchData> {
        WithCachedTxHashes {
            tx_hashes: self.tx_hashes.into(),
            inner: PreferredBatchData {
                sequence_number: self.sequence_number,
                visible_slots_to_advance: self.visible_slots_to_advance,
                data: self.txs,
            },
        }
    }
}

/// See [`PreferredSequencerDbBackend::read_completed_blobs`].
#[derive(Debug, Clone)]
pub enum PreferredSequencerReadBlob {
    Batch(PreferredSequencerReadBatch),
    Proof {
        blob_id: BlobInternalId,
        sequence_number: SequenceNumber,
        data: Arc<[u8]>,
    },
}

impl PreferredSequencerReadBlob {
    pub fn sequence_number(&self) -> SequenceNumber {
        match &self {
            PreferredSequencerReadBlob::Batch(batch) => batch.sequence_number,
            PreferredSequencerReadBlob::Proof {
                sequence_number, ..
            } => *sequence_number,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DbEvent {
    TxAccepted(FullyBakedTx, TxHash),
    BatchStarted {
        sequence_number: SequenceNumber,
        visible_slot_number_after_increase: VisibleSlotNumber,
        visible_slots_to_advance: NonZero<u8>,
    },
    BatchClosed(SequenceNumber),
    ProofBlobAccepted(SequenceNumber),
}

pub struct PreferredSequencerDb<S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    backend: Box<dyn PreferredSequencerDbBackend>,
    phantom: PhantomData<S>,
    sequence_number_of_next_blob: SequenceNumber,
    completed_blobs: VecDeque<PreferredSequencerReadBlob>,
    in_progress_batch: Option<PreferredSequencerReadBatch>,
    event_stream: Option<mpsc::Sender<DbEvent>>,
    phantom_runtime: PhantomData<Rt>,
}

impl<S, Rt> PreferredSequencerDb<S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    pub async fn new(backend: Box<dyn PreferredSequencerDbBackend>) -> anyhow::Result<Self> {
        let completed_blobs = VecDeque::from(backend.read_completed_blobs().await?);
        let in_progress_batch = backend.read_in_progress_batch().await?;

        let sequence_number_of_next_blob = match (completed_blobs.back(), &in_progress_batch) {
            (Some(blob), None) => blob.sequence_number() + 1,
            (None, Some(batch)) => batch.sequence_number + 1,
            (Some(blob), Some(batch)) => {
                std::cmp::max(blob.sequence_number(), batch.sequence_number) + 1
            }
            (None, None) => 0,
        };

        Ok(Self {
            backend,
            phantom: PhantomData,
            sequence_number_of_next_blob,
            completed_blobs,
            in_progress_batch,
            event_stream: None,
            phantom_runtime: PhantomData,
        })
    }

    pub fn next_sequence_number(&self) -> SequenceNumber {
        self.sequence_number_of_next_blob
    }

    /// Under normal operations, the sequencer will determine the next
    /// sequence number to use. When syncing, however, the DA (i.e. the node)
    /// will determine the next sequence number to use.
    pub fn overwrite_next_sequence_number(&mut self, sequence_number: SequenceNumber) {
        info!(%sequence_number, "Overwriting next sequence number");

        self.sequence_number_of_next_blob = sequence_number;
        track_sequence_number(self.sequence_number_of_next_blob);
    }

    pub fn increment_next_sequence_number(&mut self) {
        self.sequence_number_of_next_blob += 1;
        track_sequence_number(self.sequence_number_of_next_blob);
    }

    pub fn in_progress_batch_opt(&self) -> Option<&PreferredSequencerReadBatch> {
        self.in_progress_batch.as_ref()
    }

    #[tracing::instrument(skip_all, level = "trace")]
    pub async fn insert_tx(&mut self, tx: FullyBakedTx, hash: TxHash) -> anyhow::Result<()> {
        let Some(batch) = self.in_progress_batch.as_mut() else {
            panic!("No in-progress batch; this is a bug, please report it");
        };

        self.backend
            .add_tx(
                batch.sequence_number,
                batch.txs.len() as u64,
                tx.clone(),
                hash,
            )
            .await?;

        batch.txs.push(tx.clone());
        batch.tx_hashes.push(hash);

        // If there are no receivers, we don't send the tx. This is as it should be.
        self.send_event_if_necessary(DbEvent::TxAccepted(tx, hash))
            .await;

        Ok(())
    }

    async fn send_event_if_necessary(&mut self, event: DbEvent) {
        let Some(open_stream) = &self.event_stream else {
            return;
        };

        match open_stream.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(event)) => {
                // If the operation would block, print a warning before blocking.
                tracing::warn!("DbEvent stream is full, accepting txs is temporarily blocked; this means that `update_state` is taking too long to catch up causing the channel to become full. Consider bumping the db event channel size.");
                let res = open_stream.send(event).await;
                // If the receiver was dropped, we don't need to send events anymore.
                tracing::info!(
                    max_capacity = open_stream.max_capacity(),
                    remaining_capacity = open_stream.capacity(),
                    "The event stream is no longer full. accepting txs is unblocked"
                );
                if res.is_err() {
                    self.event_stream = None;
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // If the receiver was dropped, we don't need to send events anymore.
                self.event_stream = None;
            }
        }
    }

    pub async fn start_batch(
        &mut self,
        visible_slot_number_after_increase: VisibleSlotNumber,
        visible_slots_to_advance: NonZero<u8>,
    ) -> anyhow::Result<SequenceNumber> {
        assert!(
            self.in_progress_batch.is_none(),
            "There's already an in-progress batch; this is a bug, please report it"
        );

        self.debug_assert_in_progress_batch(
            "Cached in-progress batch state (None) didn't match backend db state",
        )
        .await;

        let blob_id = new_blob_id();
        let sequence_number = self.sequence_number_of_next_blob;

        tracing::debug!(
            sequence_number,
            blob_id,
            %visible_slot_number_after_increase,
            visible_slots_to_advance,
            "Storing new rollup block"
        );

        self.backend
            .begin_rollup_block(
                sequence_number,
                blob_id,
                visible_slot_number_after_increase,
                visible_slots_to_advance,
            )
            .await?;

        self.in_progress_batch = Some(PreferredSequencerReadBatch {
            sequence_number,
            visible_slot_number_after_increase,
            visible_slots_to_advance,
            blob_id,
            txs: vec![],
            tx_hashes: vec![],
        });
        self.increment_next_sequence_number();

        self.send_event_if_necessary(DbEvent::BatchStarted {
            sequence_number,
            visible_slot_number_after_increase,
            visible_slots_to_advance,
        })
        .await;

        Ok(sequence_number)
    }

    pub fn all_completed_blobs_greater_than_or_equal_to(
        &self,
        sequence_number: SequenceNumber,
    ) -> Vec<PreferredSequencerReadBlob> {
        self.completed_blobs
            .iter()
            .filter(|b| {
                // Pruning invariants say it MAY remove older blobs, but we don't know for sure.
                b.sequence_number() >= sequence_number
            })
            .cloned()
            .collect()
    }

    pub async fn insert_proof_blob(
        &mut self,
        blob_id: BlobInternalId,
        data: Arc<[u8]>,
    ) -> anyhow::Result<SequenceNumber> {
        let sequence_number = self.sequence_number_of_next_blob;

        self.backend
            .add_proof_blob(sequence_number, blob_id, data.clone())
            .await?;

        self.completed_blobs
            .push_back(PreferredSequencerReadBlob::Proof {
                blob_id,
                sequence_number,
                data,
            });
        self.increment_next_sequence_number();
        self.send_event_if_necessary(DbEvent::ProofBlobAccepted(sequence_number))
            .await;

        Ok(sequence_number)
    }

    pub async fn terminate_batch(&mut self) -> anyhow::Result<PreferredSequencerReadBatch> {
        let Some(in_progress_batch) = self.in_progress_batch.as_ref() else {
            panic!("No in-progress batch; this is a bug, please report it");
        };

        self.backend.end_rollup_block(in_progress_batch).await?;

        self.debug_assert_in_progress_batch(
            "Backend didn't remove in-progress batch from database when ending rollup block",
        )
        .await;
        let sequence_number = in_progress_batch.sequence_number;
        let batch = self
            .in_progress_batch
            .take()
            .expect("No in-progress batch; this is a bug, please report it");

        self.completed_blobs
            .push_back(PreferredSequencerReadBlob::Batch(batch.clone()));

        self.send_event_if_necessary(DbEvent::BatchClosed(sequence_number))
            .await;

        Ok(batch)
    }

    pub(super) async fn prune(
        &mut self,
        prune_up_to_including: SequenceNumber,
    ) -> anyhow::Result<()> {
        self.backend.prune(prune_up_to_including).await?;

        // We could also do binary search, but this seems fast enough.
        while let Some(blob) = self.completed_blobs.front() {
            if blob.sequence_number() > prune_up_to_including {
                break;
            }

            self.completed_blobs.pop_front();
        }

        Ok(())
    }

    pub fn subscribe_to_events(&mut self, limit: usize) -> mpsc::Receiver<DbEvent> {
        assert!(self.event_stream.is_none(), "Attempted to subscribe to sequencer events while a subscription is already open. This is a bug, please report it.");
        let (tx, rx) = mpsc::channel(limit);
        self.event_stream = Some(tx);
        rx
    }

    pub fn unsubscribe_from_events(&mut self) {
        self.event_stream = None;
    }

    async fn debug_assert_in_progress_batch(&self, msg: &str) {
        if cfg!(debug_assertions) {
            match self.backend.read_in_progress_batch().await {
                Ok(None) => {}
                other => {
                    panic!("{msg}: {other:?}");
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum StoredBlob {
    Batch {
        blob_id: BlobInternalId,
        visible_slot_number_after_increase: VisibleSlotNumber,
        visible_slots_to_advance: NonZero<u8>,
    },
    Proof {
        blob_id: BlobInternalId,
        data: Arc<[u8]>,
    },
}

pub(crate) fn latest_finalized_sequence_number<S, Rt>(
    latest_state_info: &StateUpdateInfo<S::Storage>,
    runtime: &mut Rt,
) -> Option<SequenceNumber>
where
    S: Spec,
    Rt: Runtime<S>,
{
    let mut checkpoint = StateCheckpoint::new(latest_state_info.storage.clone(), &runtime.kernel());
    let mut state = KernelStateAccessor::from_checkpoint(&runtime.kernel(), &mut checkpoint);

    state.update_true_slot_number(latest_state_info.latest_finalized_slot_number);
    runtime
        .kernel()
        .next_sequence_number(&mut state)
        .checked_sub(1)
}
