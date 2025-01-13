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

use std::marker::PhantomData;
use std::num::NonZero;
use std::path::Path;
use std::sync::Arc;

use borsh::{BorshDeserialize, BorshSerialize};
use rockbound::{gen_rocksdb_options, SchemaBatch};
use sov_blob_storage::{PreferredBatchData, PreferredProofData, SequenceNumber};
use sov_db::{
    define_table_with_seek_key_codec, define_table_without_codec, impl_borsh_value_codec,
};
use sov_modules_api::capabilities::BlobSelector;
use sov_modules_api::{KernelStateAccessor, Runtime, Spec, StateCheckpoint, StateUpdateInfo};
use tracing::{error, trace};

use crate::batch_builders::{SeqDbTx, SeqDbTxId, WithCachedTxHashes};

/// Holds data related to [`super::PreferredBatchBuilder`]:
///
///  1. The current in-progress batch.
///  2. All preferred blobs that haven't been finalized yet.
///     This is necessary because, during batch builder initialization, state is
///     restored starting from the last finalized slot.
#[derive(Debug)]
pub struct PreferredBbDb<S: Spec, R: Runtime<S>> {
    phantom: PhantomData<S>,
    runtime: R,
    db: Arc<rockbound::DB>,
    sequence_number_of_next_blob: SequenceNumber,
    /// The sequence number of the batch for which we're currently accepting
    /// transactions.
    ///
    /// This is cached for performance reasons, otherwise we'd
    /// have to hit the DB way too
    /// frequently.
    ///
    /// # Note
    /// Gotta make sure it never goes out of sync with
    /// [`InProgressBatchInfo`].
    pub sequence_number_of_in_progress_batch: Option<SequenceNumber>,
}

impl<S: Spec, R: Runtime<S>> PreferredBbDb<S, R> {
    const DB_NAME: &'static str = "preferred_batch_builder";
    const TABLES: &'static [&'static str] = &[
        tables::SingletonInProgressBatchInfo::table_name(),
        tables::NotFinalizedPreferredBlobs::table_name(),
        tables::InProgressBatchTxs::table_name(),
        tables::BatchesWaitingToBePublished::table_name(),
    ];

    /// Opens a new [`PreferredBbDb`] at the given path.
    pub async fn new(
        path: &Path,
        latest_state_info: &StateUpdateInfo<S::Storage>,
    ) -> anyhow::Result<Self> {
        let db = Arc::new(rockbound::DB::open(
            path.join(Self::DB_NAME),
            Self::DB_NAME,
            Self::TABLES.iter().copied(),
            &gen_rocksdb_options(&Default::default(), false),
        )?);

        let sequence_number_of_in_progress_batch = db
            .get::<tables::SingletonInProgressBatchInfo>(&())?
            .map(|s| s.sequence_number);

        let next_sequence_number =
            calculate_sequence_number_of_next_blob(&db, sequence_number_of_in_progress_batch)?;

        let mut db = Self {
            phantom: PhantomData,
            runtime: R::default(),
            db,
            sequence_number_of_next_blob: next_sequence_number,
            sequence_number_of_in_progress_batch,
        };

        db.recalculate_next_sequence_number(latest_state_info).await;

        Ok(db)
    }

    /// Recalculates what the next sequence number should be, based on the
    /// latest information coming from the node.
    ///
    /// # Why?
    /// Under normal operations, the [`PreferredBbDb`] contains more information
    /// about the very latest preferred blobs than the node itself. This is
    /// because the preferred sequencer stores information about batches (as
    /// well as proof blobs) before submitting them to the DA.
    ///
    /// When e.g. deploying a new preferred sequencer, however, or re-syncing
    /// the node & sequencer from scratch, the [`PreferredBbDb`] must be fed
    /// information about the sequence numbers from the node. This method does
    /// exactly that.
    pub async fn recalculate_next_sequence_number(
        &mut self,
        latest_state_info: &StateUpdateInfo<S::Storage>,
    ) {
        let mut checkpoint =
            StateCheckpoint::new(latest_state_info.storage.clone(), &self.runtime.kernel());
        let mut state =
            KernelStateAccessor::from_checkpoint(&self.runtime.kernel(), &mut checkpoint);

        // We simply pick the greatest one of the two.
        let next_sequence_number = std::cmp::max(
            self.runtime.kernel().next_sequence_number(&mut state),
            self.sequence_number_of_next_blob,
        );

        // Now, we query what the situation is as of the latest finalized
        // height. We don't care to hold data related to anything older than
        // that.
        state.update_true_slot_number(latest_state_info.latest_finalized_slot_number);

        let next_sequence_number_as_of_latest_finalized_rollup_height =
            self.runtime.kernel().next_sequence_number(&mut state);

        if let Some(latest_finalized_sequence_number) =
            next_sequence_number_as_of_latest_finalized_rollup_height.checked_sub(1)
        {
            if let Err(error) = self
                .prune_up_to_including(latest_finalized_sequence_number)
                .await
            {
                // Lack of pruning will cause storage to grow and performance to
                // degrade, but functional correctness is not affected.
                //
                // While this is worrying, it's not a critical condition and we
                // don't have to return an error. If we did, the caller would
                // have to make a decision about what to do, and chances are it
                // would either do the same thing anyway (i.e. log and continue)
                // or the wrong thing (i.e. panic/abort).
                //
                // Moreover, a single missed pruning run will not cause any
                // issues as long as some other subsequent
                // pruning run is successful, which means the node should be able to
                // recover from this error by itself.
                error!(
                    latest_finalized_sequence_number,
                    %error,
                    "Failed to prune `PreferredBbDb`; check your database integrity"
                );
            }
        }

        self.sequence_number_of_next_blob = next_sequence_number;
    }

    /// Returns all blobs stored in the database that have NOT been processed
    /// yet as of the given [`StateUpdateInfo`].
    pub async fn all_subsequent_blobs(
        &self,
        latest_state_info: &StateUpdateInfo<S::Storage>,
    ) -> anyhow::Result<Vec<PreferredBbDbBlob>> {
        let mut checkpoint =
            StateCheckpoint::new(latest_state_info.storage.clone(), &self.runtime.kernel());
        let mut state =
            KernelStateAccessor::from_checkpoint(&self.runtime.kernel(), &mut checkpoint);
        let next_sequence_number_according_to_node =
            self.runtime.kernel().next_sequence_number(&mut state);

        trace!(
            next_sequence_number_according_to_node,
            "Fetching preferred blobs"
        );

        let mut iter = self.db.iter::<tables::NotFinalizedPreferredBlobs>()?;
        iter.seek(&next_sequence_number_according_to_node)?;

        let mut blobs = vec![];
        for iter_res in iter {
            let item = iter_res?;
            let sequence_number = item.key;
            let stored_blob = item.value;

            assert!(
                sequence_number >= next_sequence_number_according_to_node,
                "This iteration has a lower bound on `next_sequence_number_according_to_node`, but one of the items has a lower sequence number. This is a logic bug; please report it"
            );

            blobs.push(stored_blob);
        }

        assert!(
            // Note: they must be ordered in ascending order by sequence number,
            // but they're not necessarily a *contiguous* range. Almost. The
            // sequence is allowed one gap due to the in-progress batch.
            blobs
                .windows(2)
                .all(|b| b[0].sequence_number() < b[1].sequence_number()),
            "Blobs stored in the database are not ordered by sequence number; this is a bug"
        );

        if let Some(seq_num_of_in_progress_batch) = self.sequence_number_of_in_progress_batch {
            assert!(blobs
                .iter()
                .filter_map(|b| match b {
                    PreferredBbDbBlob::Batch(_) => Some(b.sequence_number()),
                    _ => None,
                })
                .all(|n| n < seq_num_of_in_progress_batch),
                "All completed batches MUST have a sequence number that's lower than the in-progress batch's"
            );
        }

        Ok(blobs)
    }

    /// Inserts a transaction into the current batch.
    ///
    /// # Panics
    ///
    /// Panics if there's no in-progress batch. See [`Self::start_batch`].
    pub async fn insert_tx(&mut self, tx: &SeqDbTx) -> anyhow::Result<()> {
        assert!(
            self.sequence_number_of_in_progress_batch.is_some(),
            "Inserting tx but there's no in progress batch; this is a bug, please report it"
        );

        self.db
            .put_async::<tables::InProgressBatchTxs>(&tx.uuid_v7, tx)
            .await?;

        Ok(())
    }

    pub async fn in_progress_batch_opt(
        &self,
    ) -> anyhow::Result<Option<WithCachedTxHashes<PreferredBatchData>>> {
        let Some(info) = self.db.get::<tables::SingletonInProgressBatchInfo>(&())? else {
            return Ok(None);
        };

        let Some(sequence_number) = self.sequence_number_of_in_progress_batch else {
            return Ok(None);
        };

        assert_eq!(
            sequence_number, info.sequence_number,
            "In-memory value doesn't match the database value. This is unexpected and would cause the sequencer to misbehave. This is either a bug or a case of data corruption."
        );

        let mut txs = vec![];
        let mut tx_hashes = vec![];

        for item_res in self.db.iter::<tables::InProgressBatchTxs>()? {
            let item = item_res?;

            txs.push(item.value.tx);
            tx_hashes.push(item.value.hash);
        }

        Ok(Some(WithCachedTxHashes {
            inner: PreferredBatchData {
                sequence_number,
                data: txs,
                visible_slots_to_advance: info.visible_slots_to_advance,
            },
            tx_hashes,
        }))
    }

    /// Terminates the current in-progress batch and returns its [`SequenceNumber`].
    pub async fn terminate_batch(
        &mut self,
    ) -> anyhow::Result<WithCachedTxHashes<PreferredBatchData>> {
        let batch = self
            .in_progress_batch_opt()
            .await?
            .expect("No in-progress batch; this is a bug, please report it");
        let sequence_number = batch.inner.sequence_number;
        let blob = PreferredBbDbBlob::Batch(batch.clone());

        // DB operations.
        {
            let mut s = SchemaBatch::default();

            // Collect all tx IDs...
            let mut db_tx_ids = vec![];
            for item_res in self.db.iter::<tables::InProgressBatchTxs>()? {
                db_tx_ids.push(item_res?.key);
            }

            assert!(
                // Checks for issues related to lexicographic VS little-endian
                // sorting order. Wouldn't be the first time... or the second.
                //
                // See docs of `define_table_with_seek_key_codec` for more info.
                db_tx_ids.windows(2).all(|ids| ids[0] < ids[1]),
                "DB tx ids are not sorted in ascending order, somehow. This is a bug, please report it.",
            );

            // ...and delete them.
            for id in &db_tx_ids {
                s.delete::<tables::InProgressBatchTxs>(id)?;
            }

            s.put::<tables::NotFinalizedPreferredBlobs>(&sequence_number, &blob)?;
            s.put::<tables::BatchesWaitingToBePublished>(&sequence_number, &())?;
            s.delete::<tables::SingletonInProgressBatchInfo>(&())?;

            self.db.write_schemas_async(&s).await?;
        }

        self.sequence_number_of_in_progress_batch = None;

        Ok(batch)
    }

    /// Starts a new in-progress batch.
    pub async fn start_batch(
        &mut self,
        visible_slots_to_advance: NonZero<u8>,
    ) -> anyhow::Result<SequenceNumber> {
        assert!(
            self.sequence_number_of_in_progress_batch.is_none(),
            "There's already an in-progress batch; this is a bug, please report it"
        );

        let sequence_number = self.sequence_number_of_next_blob;

        let mut s = SchemaBatch::default();

        s.put::<tables::SingletonInProgressBatchInfo>(
            &(),
            &InProgressBatchInfo {
                sequence_number,
                visible_slots_to_advance,
            },
        )?;

        self.db.write_schemas_async(&s).await?;

        // We've written all the information we needed to the DB, but we still
        // need to update "cached" in-memory values.
        //
        // We store the sequence number of the in-progress batch...
        self.sequence_number_of_in_progress_batch = Some(sequence_number);
        // ...and we make sure the next blob will have a different sequence
        // number.
        self.sequence_number_of_next_blob += 1;

        Ok(sequence_number)
    }

    /// Assigns a [`SequenceNumber`] to a proof blob and stores it.
    pub async fn insert_proof_blob(&mut self, data: Vec<u8>) -> anyhow::Result<SequenceNumber> {
        let sequence_number = self.sequence_number_of_next_blob;

        self.db
            .put_async::<tables::NotFinalizedPreferredBlobs>(
                &sequence_number,
                &PreferredBbDbBlob::Proof(PreferredProofData {
                    data,
                    sequence_number,
                }),
            )
            .await?;
        self.sequence_number_of_next_blob += 1;

        Ok(sequence_number)
    }

    /// Returns the [`SequenceNumber`] of the oldest/earliest batch stored in
    /// this [`PreferredBbDb`] that hasn't been successfully sent to the DA yet.
    pub async fn earliest_batch_not_sent_yet(
        &self,
    ) -> anyhow::Result<Option<WithCachedTxHashes<PreferredBatchData>>> {
        let Some(sequence_number) = self
            .sequence_number_of_earliest_batch_not_sent_yet()
            .await?
        else {
            return Ok(None);
        };
        let Some(batch) = self
            .db
            .get::<tables::NotFinalizedPreferredBlobs>(&sequence_number)?
        else {
            return Ok(None);
        };

        if let PreferredBbDbBlob::Batch(batch) = batch {
            Ok(Some(batch))
        } else {
            panic!("Database error: expected to find batch, but a proof blob was found instead. Either db is corrupted or this is a bug.");
        }
    }

    /// Removes the [`SequenceNumber`] of [`PreferredBbDb::earliest_batch_not_sent_yet`].
    pub async fn advance_not_sent_yet_cursor(&mut self) -> anyhow::Result<()> {
        let Some(sequence_number) = self
            .sequence_number_of_earliest_batch_not_sent_yet()
            .await?
        else {
            return Ok(());
        };

        self.db
            .delete::<tables::BatchesWaitingToBePublished>(&sequence_number)?;

        Ok(())
    }

    async fn sequence_number_of_earliest_batch_not_sent_yet(
        &self,
    ) -> anyhow::Result<Option<SequenceNumber>> {
        let Some(item_res) = self
            .db
            .iter::<tables::BatchesWaitingToBePublished>()?
            .next()
        else {
            return Ok(None);
        };

        Ok(Some(item_res?.key))
    }

    async fn remove(&mut self, sequence_number: SequenceNumber) -> anyhow::Result<()> {
        self.db
            .delete::<tables::NotFinalizedPreferredBlobs>(&sequence_number)?;

        // We don't touch tables related to the in-progress batch because the in-progress
        // batch cannot be pruned or otherwise removed.

        Ok(())
    }

    /// Can be used to remove all kinds of information from the [`Db`] that
    /// relates to old [`SequenceNumber`]s.
    ///
    /// Note: this function **SHOULD NOT** be called unless the provided
    /// [`SequenceNumber`] has been processed already by the node and it belongs
    /// to a finalized batch.
    async fn prune_up_to_including(
        &mut self,
        prune_up_to_including: SequenceNumber,
    ) -> anyhow::Result<()> {
        if let Some(sequence_number) = self.sequence_number_of_in_progress_batch {
            // The in-progress batch cannot be pruned or otherwise removed,
            // because it's not finalized yet (as a matter of fact, it's not
            // even published yet to DA) and we're only ever supposed to prune
            // blobs that are finalized.
            assert!(sequence_number > prune_up_to_including, "`prune_up_to_including` was called with too high of a sequence number and it would result in permanent loss of node data if it were carried out. This is a bug, please report it.");
        }

        let mut iter = self.db.iter::<tables::NotFinalizedPreferredBlobs>()?;
        iter.seek_for_prev(&prune_up_to_including)?;

        let mut seq_nums_to_remove = vec![];

        for item_res in iter.rev() {
            let key = item_res?.key;

            assert!(
                key <= prune_up_to_including,
                "Loop invariant broken; this is a bug, please report it"
            );

            seq_nums_to_remove.push(key);
        }

        for n in seq_nums_to_remove {
            // We don't care about atomicity. Partial pruning is still safe.
            self.remove(n).await?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum PreferredBbDbBlob {
    Batch(WithCachedTxHashes<PreferredBatchData>),
    Proof(PreferredProofData),
}

impl PreferredBbDbBlob {
    pub fn sequence_number(&self) -> SequenceNumber {
        match self {
            Self::Batch(WithCachedTxHashes { inner, .. }) => inner.sequence_number,
            Self::Proof(PreferredProofData {
                sequence_number, ..
            }) => *sequence_number,
        }
    }

    /// Returns the number of visible slots to advance for this blob
    pub fn visible_slots_to_advance(&self) -> Option<u8> {
        match self {
            Self::Batch(WithCachedTxHashes { inner, .. }) => {
                Some(inner.visible_slots_to_advance.get())
            }
            Self::Proof(_) => None,
        }
    }
}

// Note: there's not always a batch in progress.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct InProgressBatchInfo {
    pub sequence_number: SequenceNumber,
    pub visible_slots_to_advance: NonZero<u8>,
}

fn calculate_sequence_number_of_next_blob(
    db: &rockbound::DB,
    sequence_number_of_in_progress_batch: Option<SequenceNumber>,
) -> anyhow::Result<SequenceNumber> {
    match (
        sequence_number_of_in_progress_batch,
        greatest_sequence_number_presently_stored_opt(db)?,
    ) {
        (Some(a), Some(b)) => Ok(std::cmp::max(a, b) + 1),
        (Some(a), None) | (None, Some(a)) => Ok(a + 1),
        (None, None) => Ok(0),
    }
}

fn greatest_sequence_number_presently_stored_opt(
    db: &rockbound::DB,
) -> anyhow::Result<Option<SequenceNumber>> {
    let mut iter = db.iter::<tables::NotFinalizedPreferredBlobs>()?;
    iter.seek_to_last();

    let Some(last_res) = iter.last() else {
        return Ok(None);
    };

    Ok(Some(last_res?.key))
}

mod tables {
    use sov_db::define_table_with_default_codec;

    use super::*;

    define_table_with_seek_key_codec!(
        (NotFinalizedPreferredBlobs) SequenceNumber => PreferredBbDbBlob
    );

    define_table_with_default_codec!(
        (SingletonInProgressBatchInfo) () => InProgressBatchInfo
    );

    define_table_with_seek_key_codec!(
        (InProgressBatchTxs) SeqDbTxId => SeqDbTx
    );

    define_table_with_seek_key_codec!(
        (BatchesWaitingToBePublished) SequenceNumber => ()
    );
}
