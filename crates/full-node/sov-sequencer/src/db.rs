use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use borsh::{BorshDeserialize, BorshSerialize};
use rockbound::SchemaBatch;
use sov_db::rocks_db_config::gen_rocksdb_options;
use sov_db::{
    define_table_with_seek_key_codec, define_table_without_codec, impl_borsh_value_codec,
};
use sov_modules_api::FullyBakedTx;
use sov_rollup_interface::TxHash;
use uuid::Uuid;

use crate::batch_builders::BatchBuilder;

/// Transactions within [`SequencerDb`] are identified by a monotonically
/// increasing
/// [UUIDv7](https://en.wikipedia.org/wiki/Universally_unique_identifier#Version_7_(timestamp_and_random)),
/// which is then converted to a [`u128`].
pub type SeqDbTxId = u128;
/// A database holding transactions that have been submitted to the sequencer
/// and other related data.
#[derive(Clone, Debug)]
pub struct SequencerDb {
    db: Arc<rockbound::DB>,
    entry_ttl_after_use: Duration,
}

impl SequencerDb {
    const DB_PATH_SUFFIX: &'static str = "sequencer";
    const DB_NAME: &'static str = "sequencer-db";
    const TABLES: &'static [&'static str] = &[SeqDbTxByHash::table_name()];

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
        })
    }

    /// Returns all transactions stored in the database.
    pub fn read_all(&self) -> anyhow::Result<Vec<SeqDbTx>> {
        let mut txs = vec![];
        for iter_res in self.db.iter::<SeqDbTxByHash>()? {
            let item = iter_res?;
            txs.push(item.value);
        }
        Ok(txs)
    }

    /// Deletes a group of transactions from the mempool (atomically).
    pub fn remove(&self, hashes: &[TxHash]) -> anyhow::Result<()> {
        let mut batch = SchemaBatch::new();
        for hash in hashes {
            batch.delete::<SeqDbTxByHash>(hash)?;
        }
        self.db.write_schemas(&batch)?;
        Ok(())
    }

    /// Returns the configured time-to-live of [`SequencerDb`] entries.
    pub fn ttl(&self) -> Duration {
        self.entry_ttl_after_use
    }

    /// Returns true if a transaction with the given hash is present in the database.
    pub async fn contains_tx(&self, tx_hash: &TxHash) -> anyhow::Result<bool> {
        self.db
            .get::<SeqDbTxByHash>(tx_hash)
            .map(|value| value.is_some())
    }

    /// Inserts a single transaction into the mempool.
    pub async fn insert(&self, tx: &SeqDbTx) -> anyhow::Result<()> {
        self.db.put::<SeqDbTxByHash>(&tx.hash, tx)?;
        Ok(())
    }
}

/// A transaction stored inside [`SequencerDb`].
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SeqDbTx {
    /// The encoded transaction bytes.
    tx_bytes: Vec<u8>,
    /// The hash of the transaction, as calculated by
    /// [`BatchBuilder::accept_tx`](crate::batch_builders::BatchBuilder::accept_tx).
    pub hash: TxHash,
    /// A monotonically increasing UUIDv7 counter used to order transactions by
    /// insertion time. Gaps are allowed.
    pub uuid_v7: u128,
}

impl SeqDbTx {
    /// Creates a new [`SeqDbTx`] from the given transaction bytes.
    pub fn new<Bb: BatchBuilder>(hash: TxHash, tx_input: Bb::TxInput) -> Self {
        Self {
            tx_bytes: borsh::to_vec(&tx_input).unwrap(),
            hash,
            // UUIDv7 are monotonically increasing. See here:
            // <https://github.com/uuid-rs/uuid/releases/tag/1.9.0>.
            uuid_v7: Uuid::now_v7().as_u128(),
        }
    }

    /// Decodes the transaction bytes stored in the [`SeqDbTx`] into appropriate
    /// transaction type.
    pub fn tx_input<Bb: BatchBuilder>(&self) -> Bb::TxInput {
        borsh::from_slice(&self.tx_bytes)
            .expect("Failed to deserialize stored transaction; this is a bug, please report it")
    }

    /// Returns the fully baked transaction bytes stored in the [`SeqDbTx`].
    pub fn fully_baked_tx(&self) -> FullyBakedTx {
        FullyBakedTx::new(self.tx_bytes.clone())
    }
}

define_table_with_seek_key_codec!(
    /// Accepted transactions waiting to be published as part of a batch and
    /// stored in the [`SequencerDb`], keyed by hash.
    (SeqDbTxByHash) TxHash => SeqDbTx
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
