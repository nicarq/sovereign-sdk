use std::num::NonZero;
use std::path::Path;
use std::sync::Arc;

use axum::async_trait;
use rockbound::{gen_rocksdb_options, SchemaBatch};
use sov_blob_sender::BlobInternalId;
use sov_blob_storage::SequenceNumber;
use sov_modules_api::{FullyBakedTx, TxHash, VisibleSlotNumber};

use super::{
    PreferredSequencerDbBackend, PreferredSequencerReadBatch, PreferredSequencerReadBlob,
    StoredBlob,
};

#[derive(Debug)]
pub struct RocksDbBackend {
    db: Arc<rockbound::DB>,
}

#[async_trait]
impl PreferredSequencerDbBackend for RocksDbBackend {
    #[tracing::instrument(skip_all, level = "trace")]
    async fn read_completed_blobs(&self) -> anyhow::Result<Vec<PreferredSequencerReadBlob>> {
        let mut blobs = vec![];

        // Iteration might be slow, but getters are only called during
        // sequencer initialization so it's okay.
        for item_res in self.db.iter::<tables::CompletedBlobs>()? {
            let item = item_res?;
            let sequence_number = item.key;
            let stored_blob = item.value;

            blobs.push(self.read_blob(sequence_number, stored_blob).await?);
        }

        Ok(blobs)
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn read_in_progress_batch(&self) -> anyhow::Result<Option<PreferredSequencerReadBatch>> {
        let Some((sequence_number, stored_blob)) =
            self.db.get_async::<tables::InProgressBatch>(&()).await?
        else {
            return Ok(None);
        };

        match self.read_blob(sequence_number, stored_blob).await? {
            PreferredSequencerReadBlob::Batch(batch) => Ok(Some(batch)),
            _ => panic!("In-progress batch must be a batch but is a proof blob; this is a bug, please report it"),
        }
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn begin_rollup_block(
        &mut self,
        sequence_number: SequenceNumber,
        blob_id: BlobInternalId,
        visible_slot_number_after_increase: VisibleSlotNumber,
        visible_slots_to_advance: NonZero<u8>,
    ) -> anyhow::Result<()> {
        self.db
            .put_async::<tables::InProgressBatch>(
                &(),
                &(
                    sequence_number,
                    StoredBlob::Batch {
                        visible_slot_number_after_increase,
                        visible_slots_to_advance,
                        blob_id,
                    },
                ),
            )
            .await?;

        Ok(())
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn add_tx(
        &mut self,
        sequence_number: SequenceNumber,
        tx_idx_within_batch: u64,
        tx: FullyBakedTx,
        hash: TxHash,
    ) -> anyhow::Result<()> {
        self.db
            .put_async::<tables::BatchContents>(
                &(sequence_number, tx_idx_within_batch),
                &(hash, tx),
            )
            .await?;

        Ok(())
    }

    async fn pop_tx(
        &mut self,
        sequence_number_of_in_progress_batch: SequenceNumber,
        tx_idx_within_batch: u64,
    ) -> anyhow::Result<()> {
        self.db
            .delete_async::<tables::BatchContents>(&(
                sequence_number_of_in_progress_batch,
                tx_idx_within_batch,
            ))
            .await?;

        Ok(())
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn end_rollup_block(
        &mut self,
        in_progress_batch: &PreferredSequencerReadBatch,
    ) -> anyhow::Result<()> {
        let mut s = SchemaBatch::new();
        s.delete::<tables::InProgressBatch>(&())?;
        s.put::<tables::CompletedBlobs>(
            &in_progress_batch.sequence_number,
            &StoredBlob::Batch {
                blob_id: in_progress_batch.blob_id,
                visible_slots_to_advance: in_progress_batch.visible_slots_to_advance,
                visible_slot_number_after_increase: in_progress_batch
                    .visible_slot_number_after_increase,
            },
        )?;
        self.db.write_schemas_async(&s).await?;

        Ok(())
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn prune(&mut self, prune_up_to_including: SequenceNumber) -> anyhow::Result<()> {
        // We first delete blob data, and only then batch contents. We'd rather have orphaned
        // data (no harm in that, it'll just get pruned eventually) than batches
        // incorrectly marked as empty.
        //
        // Alternatively, a cross-column-family atomic delete would also work.

        self.db.delete_range::<tables::CompletedBlobs>(
            &SequenceNumber::MIN,
            // The upper bound is exclusive.
            &prune_up_to_including.saturating_add(1),
        )?;

        self.db.delete_range::<tables::BatchContents>(
            &(SequenceNumber::MIN, u64::MIN),
            &(prune_up_to_including, u64::MAX),
        )?;

        Ok(())
    }

    #[tracing::instrument(skip_all, level = "trace")]
    async fn add_proof_blob(
        &mut self,
        sequence_number: SequenceNumber,
        blob_id: BlobInternalId,
        data: Arc<[u8]>,
    ) -> anyhow::Result<()> {
        self.db
            .put_async::<tables::CompletedBlobs>(
                &sequence_number,
                &StoredBlob::Proof { data, blob_id },
            )
            .await?;
        Ok(())
    }
}

impl RocksDbBackend {
    const DB_NAME: &'static str = "preferred_sequencer";
    const TABLES: &'static [&'static str] = &[
        tables::CompletedBlobs::table_name(),
        tables::InProgressBatch::table_name(),
        tables::BatchContents::table_name(),
    ];

    /// Opens a new [`RocksDbBackend`] at the given path.
    pub async fn new(path: &Path) -> anyhow::Result<Self> {
        let db = Arc::new(rockbound::DB::open(
            path.join(Self::DB_NAME),
            Self::DB_NAME,
            Self::TABLES.iter().copied(),
            &gen_rocksdb_options(&Default::default(), false),
        )?);

        Ok(Self { db })
    }

    async fn read_blob(
        &self,
        sequence_number: SequenceNumber,
        stored_blob: StoredBlob,
    ) -> anyhow::Result<PreferredSequencerReadBlob> {
        Ok(match stored_blob {
            StoredBlob::Batch {
                visible_slot_number_after_increase,
                visible_slots_to_advance,
                blob_id,
            } => {
                let mut txs = vec![];
                let mut tx_hashes = vec![];

                // Iteration might be slow, but getters are only called during
                // sequencer initialization so it's okay.
                let mut iter = self.db.iter::<tables::BatchContents>()?;
                iter.seek(&(sequence_number, u64::MIN))?;

                for item_res in iter {
                    let item = item_res?;
                    if item.key.0 != sequence_number {
                        break;
                    }
                    let (tx_hash, tx) = item.value;
                    txs.push(tx);
                    tx_hashes.push(tx_hash);
                }

                PreferredSequencerReadBlob::Batch(PreferredSequencerReadBatch {
                    sequence_number,
                    visible_slot_number_after_increase,
                    visible_slots_to_advance,
                    txs,
                    tx_hashes,
                    blob_id,
                })
            }
            StoredBlob::Proof { data, blob_id } => PreferredSequencerReadBlob::Proof {
                sequence_number,
                data,
                blob_id,
            },
        })
    }
}

mod tables {
    use sov_db::{
        define_table_with_default_codec, define_table_with_seek_key_codec,
        define_table_without_codec, impl_borsh_value_codec,
    };

    use super::*;
    use crate::preferred::db::StoredBlob;

    define_table_with_seek_key_codec!(
        (CompletedBlobs) SequenceNumber => StoredBlob
    );

    define_table_with_default_codec!(
        (InProgressBatch) () => (SequenceNumber, StoredBlob)
    );

    define_table_with_seek_key_codec!(
        (BatchContents) (SequenceNumber, u64) => (TxHash, FullyBakedTx)
    );
}
