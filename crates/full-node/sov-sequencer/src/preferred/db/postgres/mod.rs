use std::num::NonZero;
use std::sync::Arc;

use axum::async_trait;
use sov_blob_sender::BlobInternalId;
use sov_blob_storage::SequenceNumber;
use sov_modules_api::{FullyBakedTx, TxHash};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Postgres;

use super::{
    PreferredSequencerDbBackend, PreferredSequencerReadBatch, PreferredSequencerReadBlob,
    StoredBlob,
};

pub struct PostgresBackend {
    pool: PgPool,
}

impl PostgresBackend {
    pub async fn connect(connection_string: &str) -> anyhow::Result<Self> {
        let pool = PgPoolOptions::default().connect(connection_string).await?;

        sqlx::migrate!("src/preferred/db/postgres/migrations")
            .run(&pool)
            .await?;

        Ok(Self { pool })
    }

    async fn read_blob(
        &self,
        sequence_number: SequenceNumber,
        stored_blob: StoredBlob,
    ) -> anyhow::Result<PreferredSequencerReadBlob> {
        match stored_blob {
            StoredBlob::Batch {
                visible_slot_number_after_increase,
                visible_slots_to_advance,
                blob_id,
            } => {
                let tx_rows: Vec<(Vec<u8>, Vec<u8>)> = sqlx::query_as::<Postgres, _>(
                    "SELECT hash, data FROM txs WHERE sequence_number = $1 ORDER BY batch_index",
                )
                .bind(i64::try_from(sequence_number)?)
                .fetch_all(&self.pool)
                .await?;

                let (tx_hashes, txs): (Vec<_>, Vec<_>) = tx_rows.into_iter().unzip();
                let tx_hashes = tx_hashes
                    .into_iter()
                    .map(|bytes| {
                        Ok(TxHash::new(bytes.try_into().map_err(|err| {
                            anyhow::anyhow!("Invalid database data for tx hash; check your database integrity: {err:?}")
                        })?))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let txs = txs.into_iter().map(FullyBakedTx::new).collect::<Vec<_>>();

                Ok(PreferredSequencerReadBlob::Batch(
                    PreferredSequencerReadBatch {
                        sequence_number,
                        visible_slot_number_after_increase,
                        visible_slots_to_advance,
                        txs,
                        tx_hashes,
                        blob_id,
                    },
                ))
            }
            StoredBlob::Proof { data, blob_id } => Ok(PreferredSequencerReadBlob::Proof {
                sequence_number,
                blob_id,
                data,
            }),
        }
    }
}

#[async_trait]
impl PreferredSequencerDbBackend for PostgresBackend {
    async fn begin_rollup_block(
        &mut self,
        sequence_number: SequenceNumber,
        blob_id: BlobInternalId,
        visible_slot_number_after_increase: sov_modules_api::VisibleSlotNumber,
        visible_slots_to_advance: NonZero<u8>,
    ) -> anyhow::Result<()> {
        sqlx::query::<Postgres>(
            "INSERT INTO in_progress_batch (sequence_number, borsh_value) VALUES ($1, $2)",
        )
        .bind(i64::try_from(sequence_number)?)
        .bind::<&[u8]>(
            borsh::to_vec(&StoredBlob::Batch {
                blob_id,
                visible_slot_number_after_increase,
                visible_slots_to_advance,
            })?
            .as_ref(),
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_tx(
        &mut self,
        sequence_number: SequenceNumber,
        tx_index_within_batch: u64,
        tx: FullyBakedTx,
        hash: TxHash,
    ) -> anyhow::Result<()> {
        sqlx::query::<Postgres>(
            "INSERT INTO txs (sequence_number, batch_index, hash, data) VALUES ($1, $2, $3, $4)",
        )
        .bind(i64::try_from(sequence_number)?)
        .bind(i64::try_from(tx_index_within_batch)?)
        .bind::<&[u8]>(hash.as_ref())
        .bind(tx.data)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn pop_tx(
        &mut self,
        sequence_number_of_in_progress_batch: SequenceNumber,
        tx_idx_within_batch: u64,
    ) -> anyhow::Result<()> {
        let result = sqlx::query::<Postgres>(
            "DELETE FROM txs WHERE sequence_number = $1 AND batch_index = $2",
        )
        .bind(i64::try_from(sequence_number_of_in_progress_batch)?)
        .bind(i64::try_from(tx_idx_within_batch)?)
        .execute(&self.pool)
        .await?;

        assert_eq!(result.rows_affected(), 1, "Sanity check failed. Popping tx but no rows were affected. This is a bug, please report it.");

        Ok(())
    }

    async fn end_rollup_block(
        &mut self,
        cached: &super::PreferredSequencerReadBatch,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query::<Postgres>("DELETE FROM in_progress_batch")
            .execute(&mut *tx)
            .await?;

        sqlx::query::<Postgres>("INSERT INTO blobs (sequence_number, borsh_value) VALUES ($1, $2)")
            .bind(i64::try_from(cached.sequence_number)?)
            .bind::<&[u8]>(
                borsh::to_vec(&StoredBlob::Batch {
                    blob_id: cached.blob_id,
                    visible_slots_to_advance: cached.visible_slots_to_advance,
                    visible_slot_number_after_increase: cached.visible_slot_number_after_increase,
                })?
                .as_ref(),
            )
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        Ok(())
    }

    async fn prune(&mut self, up_to_including: SequenceNumber) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query::<Postgres>(r#"DELETE FROM blobs WHERE sequence_number <= $1"#)
            .bind(i64::try_from(up_to_including)?)
            .execute(&mut *tx)
            .await?;
        sqlx::query::<Postgres>(r#"DELETE FROM txs WHERE sequence_number <= $1"#)
            .bind(i64::try_from(up_to_including)?)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        Ok(())
    }

    async fn add_proof_blob(
        &mut self,
        sequence_number: SequenceNumber,
        blob_id: BlobInternalId,
        data: Arc<[u8]>,
    ) -> anyhow::Result<()> {
        sqlx::query::<Postgres>("INSERT INTO blobs (sequence_number, borsh_value) VALUES ($1, $2)")
            .bind(i64::try_from(sequence_number)?)
            .bind::<&[u8]>(borsh::to_vec(&StoredBlob::Proof { data, blob_id })?.as_ref())
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn read_completed_blobs(&self) -> anyhow::Result<Vec<PreferredSequencerReadBlob>> {
        let stored_blobs: Vec<(i64, Vec<u8>)> = sqlx::query_as::<Postgres, _>(
            "SELECT sequence_number, borsh_value FROM blobs ORDER BY sequence_number",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut blobs = vec![];
        for (sequence_number, stored_blob_serialized) in stored_blobs {
            let sequence_number = SequenceNumber::try_from(sequence_number)?;
            let stored_blob = borsh::from_slice(&stored_blob_serialized)?;

            blobs.push(self.read_blob(sequence_number, stored_blob).await?);
        }

        Ok(blobs)
    }

    async fn read_in_progress_batch(
        &self,
    ) -> anyhow::Result<Option<super::PreferredSequencerReadBatch>> {
        let Some((sequence_number, stored_blob_serialized)): Option<(i64, Vec<u8>)> =
            sqlx::query_as::<Postgres, _>(
                "SELECT sequence_number, borsh_value FROM in_progress_batch",
            )
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };

        let sequence_number = SequenceNumber::try_from(sequence_number)?;
        let stored_blob = borsh::from_slice(&stored_blob_serialized)?;

        match self.read_blob(sequence_number, stored_blob).await? {
            PreferredSequencerReadBlob::Batch(batch) => Ok(Some(batch)),
            PreferredSequencerReadBlob::Proof { .. } => panic!(
                "Expected a batch blob, but got a proof blob. This is a bug, please report it"
            ),
        }
    }
}
