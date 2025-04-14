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
use futures::stream::BoxStream;
use futures::StreamExt;
use sov_blob_sender::{new_blob_id, BlobInternalId};
use sov_blob_storage::{PreferredBatchData, SequenceNumber};
use sov_modules_api::capabilities::BlobSelector;
use sov_modules_api::{
    FullyBakedTx, KernelStateAccessor, Runtime, Spec, StateCheckpoint, StateUpdateInfo, TxHash,
    VisibleSlotNumber,
};

use crate::common::WithCachedTxHashes;

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

    async fn read_completed_blobs(
        &self,
    ) -> anyhow::Result<BoxStream<anyhow::Result<PreferredSequencerReadBlob>>>;

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

impl From<PreferredSequencerReadBatch> for WithCachedTxHashes<PreferredBatchData> {
    fn from(batch: PreferredSequencerReadBatch) -> Self {
        WithCachedTxHashes {
            tx_hashes: batch.tx_hashes.into(),
            inner: PreferredBatchData {
                sequence_number: batch.sequence_number,
                visible_slots_to_advance: batch.visible_slots_to_advance,
                data: batch.txs,
            },
        }
    }
}

/// See [`PreferredSequencerDbBackend::read_completed_blobs`].
#[derive(Debug, Clone)]
pub enum PreferredSequencerReadBlob {
    Batch(PreferredSequencerReadBatch),
    Proof {
        #[allow(dead_code)] // TODO(@neysofu): use it to re-submit blobs upon sequencer restart.
        blob_id: BlobInternalId,
        sequence_number: SequenceNumber,
        #[allow(dead_code)]
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

pub struct PreferredSequencerDb<S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    backend: Box<dyn PreferredSequencerDbBackend>,
    phantom: PhantomData<S>,
    runtime: Rt,
    sequence_number_of_next_blob: SequenceNumber,
    completed_blobs: VecDeque<PreferredSequencerReadBlob>,
    in_progress_batch: Option<PreferredSequencerReadBatch>,
}

impl<S, Rt> PreferredSequencerDb<S, Rt>
where
    S: Spec,
    Rt: Runtime<S>,
{
    pub async fn new(backend: Box<dyn PreferredSequencerDbBackend>) -> anyhow::Result<Self> {
        let mut completed_blobs = VecDeque::new();

        let mut iter = backend.read_completed_blobs().await?;
        while let Some(blob_res) = iter.next().await {
            completed_blobs.push_back(blob_res?);
        }
        drop(iter);

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
            runtime: Default::default(),
            sequence_number_of_next_blob,
            completed_blobs,
            in_progress_batch,
        })
    }

    pub async fn in_progress_batch_opt(
        &self,
    ) -> anyhow::Result<Option<&PreferredSequencerReadBatch>> {
        Ok(self.in_progress_batch.as_ref())
    }

    #[tracing::instrument(skip_all, level = "trace")]
    pub async fn insert_tx(&mut self, tx: FullyBakedTx, hash: TxHash) -> anyhow::Result<()> {
        let Some(in_progress_batch) = self.in_progress_batch.as_ref() else {
            panic!("No in-progress batch; this is a bug, please report it");
        };

        self.backend
            .add_tx(
                in_progress_batch.sequence_number,
                in_progress_batch.txs.len() as u64,
                tx.clone(),
                hash,
            )
            .await?;
        let batch = self
            .in_progress_batch
            .as_mut()
            .expect("No in-progress batch; this is a bug, please report it");

        batch.txs.push(tx);
        batch.tx_hashes.push(hash);

        Ok(())
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
        debug_assert!(
            matches!(self.backend.read_in_progress_batch().await, Ok(None)),
            "Cached in-progress batch state (None) didn't match backend db state (Some)"
        );

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
        self.sequence_number_of_next_blob += 1;

        Ok(sequence_number)
    }

    pub async fn subsequent_completed_blobs(
        &mut self,
        latest_state_info: &StateUpdateInfo<S::Storage>,
    ) -> anyhow::Result<Vec<PreferredSequencerReadBlob>> {
        let next_sequence_number_as_of_latest_finalized_rollup_height = {
            let mut checkpoint =
                StateCheckpoint::new(latest_state_info.storage.clone(), &self.runtime.kernel());
            let mut state =
                KernelStateAccessor::from_checkpoint(&self.runtime.kernel(), &mut checkpoint);

            // Now, we query what the situation is as of the latest finalized
            // height. We don't care to hold data related to anything older than
            // that.
            state.update_true_slot_number(latest_state_info.latest_finalized_slot_number);
            self.runtime.kernel().next_sequence_number(&mut state)
        };

        // Now is as good a time as any to prune old blobs that are no longer needed.
        match next_sequence_number_as_of_latest_finalized_rollup_height.checked_sub(1) {
            Some(last_finalized_sequence_number) => {
                self.prune(last_finalized_sequence_number).await?;
            }
            None => {
                // Nothing to prune because there's no sequence number history.
            }
        }

        Ok(self
            .completed_blobs
            .iter()
            .filter(|b| {
                // Pruning invariants say it MAY remove older blobs, but we don't know for sure.
                b.sequence_number() >= next_sequence_number_as_of_latest_finalized_rollup_height
            })
            .cloned()
            .collect())
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
        self.sequence_number_of_next_blob += 1;

        Ok(sequence_number)
    }

    pub async fn terminate_batch(&mut self) -> anyhow::Result<PreferredSequencerReadBatch> {
        let Some(in_progress_batch) = self.in_progress_batch.as_ref() else {
            panic!("No in-progress batch; this is a bug, please report it");
        };

        self.backend.end_rollup_block(in_progress_batch).await?;
        debug_assert!(
            matches!(self.backend.read_in_progress_batch().await, Ok(None)),
            "Backend didn't remove in-progress batch from database when ending rollup block"
        );
        let batch = self
            .in_progress_batch
            .take()
            .expect("No in-progress batch; this is a bug, please report it");
        self.completed_blobs
            .push_back(PreferredSequencerReadBlob::Batch(batch.clone()));

        Ok(batch)
    }

    async fn prune(&mut self, prune_up_to_including: SequenceNumber) -> anyhow::Result<()> {
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
