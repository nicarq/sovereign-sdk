use std::path::Path;
use std::sync::Arc;

use borsh::{BorshDeserialize, BorshSerialize};
use sov_rollup_interface::services::batch_builder::TxHash;

use crate::rocks_db_config::gen_rocksdb_options;
use crate::schema::tables::{TxByIncrId, TxIncrIdByHash};
use crate::schema::types::TxIncrId;

/// A database holding transactions that have been submitted to the sequencer
/// and other related data.
#[derive(Clone, Debug)]
pub struct SequencerDB {
    db: Arc<rockbound::DB>,
    next_tx_id: TxIncrId,
    txs_count: usize,
}

impl SequencerDB {
    const DB_PATH_SUFFIX: &'static str = "mempool";
    const DB_NAME: &'static str = "mempool-db";

    const TABLES: &'static [&'static str] =
        &[TxByIncrId::table_name(), TxIncrIdByHash::table_name()];

    /// Initializes a new [`SequencerDB`] at the given path.
    pub fn new(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().join(Self::DB_PATH_SUFFIX);
        let db = rockbound::DB::open(
            path,
            Self::DB_NAME,
            Self::TABLES.iter().copied(),
            &gen_rocksdb_options(&Default::default(), false),
        )?;

        // We immediately calculate the next transaction ID. This is either:
        // - 0 if the database is empty.
        // - The highest transaction ID in the database, plus one.
        let next_tx_id = largest_tx_id(&db)?.map(|id| id + 1).unwrap_or_default();
        // The next transaction ID should not currently be in use.
        assert!(db.get::<TxByIncrId>(&next_tx_id)?.is_none());

        // This is potentially problematic because it iterates over the entire
        // column family. RocksDB unfortunately doesn't expose accurate item
        // counts.
        // Another potential solution might be to limit the number of items
        // returned by the iterator and error out if the count is too high.
        let txs_count = count_items(&db)?;

        Ok(Self {
            db: Arc::new(db),
            next_tx_id,
            txs_count,
        })
    }

    /// Returns the number of transactions currently stored inside the [`SequencerDB`].
    pub fn txs_count(&self) -> usize {
        self.txs_count
    }

    /// Removes and returns the least recently added transaction from the
    /// [`SequencerDB`].
    pub fn pop(&mut self) -> anyhow::Result<Option<MempoolTx>> {
        let Some((smallest_incr_id, tx)) = self
            .db
            .iter::<TxByIncrId>()?
            .next()
            .transpose()?
            .map(|data| (data.key, data.value))
        else {
            return Ok(None);
        };

        self.db.delete::<TxByIncrId>(&smallest_incr_id)?;
        self.db.delete::<TxIncrIdByHash>(&tx.hash)?;
        self.txs_count -= 1;

        Ok(Some(tx))
    }

    /// Puts a transaction back into the [`SequencerDB`] after popping it. Its
    /// priority is unchanged and it will be returned by the next call to
    /// [`SequencerDB::pop`].
    ///
    /// # Panics
    /// Will panic if not called immediately after [`SequencerDB::pop`] for the
    /// same transaction.
    pub fn reinsert(&mut self, tx: MempoolTx) -> anyhow::Result<()> {
        // We must create a transaction ID that will be lower than all the
        // others. We do that by finding the smallest one and subtracting one,
        // or 0 if none is found.
        let incr_id = self
            .smallest_incr_id()?
            .map(|TxIncrId(id)| {
                TxIncrId(id.checked_sub(1).expect(
                    "ID underflow, only possible if reinsert was called without popping first",
                ))
            })
            .unwrap_or_default();

        assert!(incr_id < self.next_tx_id, "ID inconsistency detected; this is the result of a bug due to incorrect usage of reinsert");

        self.db.put::<TxByIncrId>(&incr_id, &tx)?;
        self.db.put::<TxIncrIdByHash>(&tx.hash, &incr_id)?;
        self.txs_count += 1;

        Ok(())
    }

    /// Adds a transaction to the [`SequencerDB`].
    pub fn push(&mut self, tx: MempoolTx) -> anyhow::Result<(TxHash, TxIncrId)> {
        if self.db.get::<TxIncrIdByHash>(&tx.hash)?.is_some() {
            return Err(anyhow::anyhow!(
                "Mempool already contains tx with hash {:?}",
                tx.hash
            ));
        }

        self.db.put::<TxIncrIdByHash>(&tx.hash, &self.next_tx_id)?;
        self.db.put::<TxByIncrId>(&self.next_tx_id, &tx)?;
        self.txs_count += 1;

        let tx_id = self.next_tx_id;
        self.next_tx_id += 1;

        Ok((tx.hash, tx_id))
    }

    /// Checks whether a transaction with the given hash is stored in the
    /// [`SequencerDB`].
    pub fn contains(&self, tx_hash: &TxHash) -> anyhow::Result<bool> {
        Ok(self.db.get::<TxIncrIdByHash>(tx_hash)?.is_some())
    }

    fn smallest_incr_id(&self) -> anyhow::Result<Option<TxIncrId>> {
        Ok(self
            .db
            .iter::<TxByIncrId>()?
            .next()
            .transpose()?
            .map(|data| data.key))
    }
}

fn largest_tx_id(db: &rockbound::DB) -> anyhow::Result<Option<TxIncrId>> {
    Ok(db
        .iter::<TxByIncrId>()?
        .rev()
        .next()
        .transpose()?
        .map(|data| data.key))
}

fn count_items(db: &rockbound::DB) -> anyhow::Result<usize> {
    Ok(db.iter::<TxByIncrId>()?.count())
}

/// A transaction as stored inside [`SequencerDB`].
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct MempoolTx {
    /// The transaction hash.
    pub hash: TxHash,
    /// The raw, unmodified transaction bytes.
    pub tx_bytes: Vec<u8>,
    /// The runtime message of the transaction, which was extracted from
    /// [`MempoolTx::tx_bytes`] when the transaction was added to the mempool.
    pub runtime_msg: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_tx(hash_byte: u8) -> MempoolTx {
        MempoolTx {
            hash: [hash_byte; 32],
            tx_bytes: vec![1, 2, 3],
            runtime_msg: vec![1, 2, 3],
        }
    }

    #[test]
    fn basic_mempool_operations() {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path();
        let mut db = SequencerDB::new(path).unwrap();

        let tx = mock_tx(1);
        let hash = tx.hash;

        // The tx is not in the mempool yet!
        assert!(!db.contains(&hash).unwrap());
        assert_eq!(db.txs_count, 0);

        db.push(tx.clone()).unwrap();

        // After pushing, we expect the tx to be in the mempool.
        assert!(db.contains(&hash).unwrap());
        assert_eq!(db.txs_count, 1);

        // After popping, the mempool should not contain the tx.
        let popped = db.pop().unwrap();
        assert_eq!(popped, Some(tx));
        assert_eq!(db.txs_count, 0);
        assert!(!db.contains(&hash).unwrap());
    }

    #[test]
    fn reinsert_incr_id_calculation() {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path();
        let mut db = SequencerDB::new(path).unwrap();

        let tx1 = mock_tx(1);
        let tx2 = mock_tx(2);

        // Push tx1, pop tx1, reinsert tx1, push tx2.
        db.push(tx1.clone()).unwrap();
        let tx1_popped = db.pop().unwrap().unwrap();
        db.reinsert(tx1_popped).unwrap();
        db.push(tx2.clone()).unwrap();

        // Next pop should be tx1, not tx2.
        assert_eq!(db.pop().unwrap().unwrap(), tx1);
    }
}
