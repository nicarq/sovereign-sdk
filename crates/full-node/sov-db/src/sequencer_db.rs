//! Database for sequencer-related data.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use borsh::{BorshDeserialize, BorshSerialize};
use rockbound::SchemaBatch;
use sov_rollup_interface::TxHash;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::rocks_db_config::gen_rocksdb_options;
use crate::{
    define_table_with_default_codec, define_table_with_seek_key_codec, define_table_without_codec,
    impl_borsh_value_codec,
};

/// Transactions within [`SequencerDb`] are identified by a monotonically
/// increasing
/// [UUIDv7](https://en.wikipedia.org/wiki/Universally_unique_identifier#Version_7_(timestamp_and_random)),
/// which is then converted to a [`u128`].
pub type SeqDbTxId = u128;

/// Incremental sequence number assigned to each batch. Used by the preferred
/// sequencer, ignored by the standard sequencer.
pub type SequenceNumber = u64;

/// A database holding transactions that have been submitted to the sequencer
/// and other related data.
#[derive(Clone, Debug)]
pub struct SequencerDb {
    db: Arc<rockbound::DB>,
    entry_ttl_after_use: Duration,
    seq_number_lock: Arc<Mutex<()>>,
}

impl SequencerDb {
    const DB_PATH_SUFFIX: &'static str = "sequencer";
    const DB_NAME: &'static str = "sequencer-db";
    const TABLES: &'static [&'static str] = &[
        AcceptedTxs::table_name(),
        AcceptedTxsByHash::table_name(),
        NextSequenceNumber::table_name(),
    ];

    /// Initializes a new [`SequencerDb`] at the given path.
    pub fn new(path: impl AsRef<Path>, entry_ttl_after_use: Duration) -> anyhow::Result<Self> {
        let path = path.as_ref().join(Self::DB_PATH_SUFFIX);
        let db = rockbound::DB::open_with_ttl(
            path,
            Self::DB_NAME,
            Self::TABLES.iter().copied(),
            &gen_rocksdb_options(&Default::default(), false),
            entry_ttl_after_use,
        )?;

        Ok(Self {
            db: Arc::new(db),
            entry_ttl_after_use,
            seq_number_lock: Default::default(),
        })
    }

    /// Returns all transactions stored in the database.
    pub fn read_all(&self) -> anyhow::Result<Vec<SeqDbTx>> {
        let mut txs = vec![];
        for iter_res in self.db.iter::<AcceptedTxs>()? {
            let item = iter_res?;
            txs.push(item.value);
        }
        Ok(txs)
    }

    /// Deletes a group of transactions from the mempool (atomically).
    pub fn remove(&self, hashes: &[TxHash]) -> anyhow::Result<()> {
        let mut batch = SchemaBatch::new();
        for hash in hashes {
            if let Some(id) = self.db.get::<AcceptedTxsByHash>(hash)? {
                batch.delete::<AcceptedTxs>(&id)?;
                batch.delete::<AcceptedTxsByHash>(hash)?;
            }
        }
        self.db.write_schemas(&batch)?;
        Ok(())
    }

    /// Returns the configured time-to-live of [`SequencerDb`] entries.
    pub fn ttl(&self) -> Duration {
        self.entry_ttl_after_use
    }

    /// Returns true if a transaction with the given hash is present in the database.
    pub async fn get(&self, tx_hash: &TxHash) -> anyhow::Result<Option<SeqDbTx>> {
        let id = self.db.get::<AcceptedTxsByHash>(tx_hash)?;

        if let Some(id) = id {
            Ok(self.db.get::<AcceptedTxs>(&id)?)
        } else {
            Ok(None)
        }
    }

    /// Inserts a single transaction into the mempool.
    pub async fn insert(&self, tx: &SeqDbTx) -> anyhow::Result<()> {
        let mut batch = SchemaBatch::new();

        batch.put::<AcceptedTxsByHash>(&tx.hash, &tx.uuid_v7)?;
        batch.put::<AcceptedTxs>(&tx.uuid_v7, tx)?;

        self.db.write_schemas(&batch)?;
        Ok(())
    }

    /// Returns the next sequence number to use for serializing preferred
    /// sequencer blobs. Subsequent calls to this method will return a different
    /// (higher) next sequence number.
    pub async fn get_and_increase_next_sequence_number(&self) -> anyhow::Result<u64> {
        let _lock = self.seq_number_lock.lock().await;
        let next_sequence_number = self.db.get::<NextSequenceNumber>(&())?.unwrap_or(0);
        self.db
            .put::<NextSequenceNumber>(&(), &(next_sequence_number + 1))?;

        Ok(next_sequence_number)
    }
}

/// A transaction stored inside [`SequencerDb`].
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SeqDbTx {
    /// The encoded transaction bytes.
    pub tx_bytes: Vec<u8>,
    /// The hash of the transaction, as calculated by
    /// the batch builder.
    pub hash: TxHash,
    /// A monotonically increasing UUIDv7 counter used to order transactions by
    /// insertion time. Gaps are allowed.
    pub uuid_v7: u128,
}

impl SeqDbTx {
    /// Creates a new [`SeqDbTx`] from the given transaction bytes.
    pub fn new_with_tx_bytes(hash: TxHash, tx_bytes: Vec<u8>) -> Self {
        Self {
            tx_bytes,
            hash,
            // UUIDv7 are monotonically increasing. See here:
            // <https://github.com/uuid-rs/uuid/releases/tag/1.9.0>.
            uuid_v7: Uuid::now_v7().as_u128(),
        }
    }
}

define_table_with_seek_key_codec!(
    /// Accepted transactions waiting to be published as part of a batch and
    /// stored in the [`SequencerDb`], keyed by hash.
    (AcceptedTxs) SeqDbTxId => SeqDbTx
);

define_table_with_seek_key_codec!(
    /// Accepted transactions waiting to be published as part of a batch and
    /// stored in the [`SequencerDb`], keyed by hash.
    (AcceptedTxsByHash) TxHash => SeqDbTxId
);

define_table_with_default_codec!(
    /// Next sequence number to use for serializing preferred sequencer blobs.
    (NextSequenceNumber) () => u64
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_v7_is_monotonically_increasing() {
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();
        assert!(a < b);
    }
}
