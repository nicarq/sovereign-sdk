use std::path::Path;
use std::sync::Arc;

use rockbound::gen_rocksdb_options;
use sov_db::{define_table_with_default_codec, define_table_without_codec, impl_borsh_value_codec};
use sov_rollup_interface::TxHash;

use crate::batch_builders::SeqDbTx;

/// A database for [`crate::batch_builders::StandardBatchBuilder`] and its
/// mempool.
#[derive(Clone, Debug)]
pub struct StandardBbDb {
    db: Arc<rockbound::DB>,
}

impl StandardBbDb {
    const TABLES: &'static [&'static str] = &[MempoolTxs::table_name()];
    const DB_NAME: &'static str = "standard_batch_builder";

    /// Initializes a new [`StandardBbDb`] at the given path.
    pub async fn new(path: &Path) -> anyhow::Result<Self> {
        let db = Arc::new(rockbound::DB::open(
            path.join(Self::DB_NAME),
            Self::DB_NAME,
            Self::TABLES.iter().copied(),
            &gen_rocksdb_options(&Default::default(), false),
        )?);

        Ok(Self { db })
    }

    /// Returns all transactions stored in the database, ordered by
    /// [`SeqDbTxId`] (i.e. tx creation time).
    pub fn read_all(&self) -> anyhow::Result<Vec<SeqDbTx>> {
        let mut txs = vec![];
        for iter_res in self.db.iter::<MempoolTxs>()? {
            let item = iter_res?;
            txs.push(item.value);
        }
        Ok(txs)
    }

    /// Deletes a transaction.
    pub fn remove(&self, hash: TxHash) -> anyhow::Result<()> {
        self.db.delete::<MempoolTxs>(&hash)?;

        Ok(())
    }

    /// Inserts a single transaction into the mempool.
    pub fn insert(&self, tx: &SeqDbTx) -> anyhow::Result<()> {
        self.db.put::<MempoolTxs>(&tx.hash, tx)?;
        Ok(())
    }
}

define_table_with_default_codec!(
    (MempoolTxs) TxHash => SeqDbTx
);
