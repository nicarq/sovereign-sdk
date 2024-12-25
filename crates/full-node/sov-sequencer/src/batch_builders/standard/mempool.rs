use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::num::NonZero;
use std::ops::Bound;
use std::sync::Arc;

use sov_modules_api::{FullyBakedTx, Spec};
use sov_rollup_interface::common::HexString;
use sov_rollup_interface::TxHash;
use tracing::debug;

use crate::batch_builders::BatchBuilder;
use crate::{SeqDbTx, SeqDbTxExtend, SeqDbTxId, TxStatus, TxStatusManager};

// mempool picks transactions in this order:
// - next_priority
// - gas per byte
#[derive(derivative::Derivative)]
#[derivative(Debug)]
pub struct Mempool<Bb: BatchBuilder> {
    max_txs_count: NonZero<usize>,
    txsm: TxStatusManager<<<Bb as BatchBuilder>::Spec as Spec>::Da>,
    // Transaction data
    // ----------------
    txs_ordered_by_most_fair_fit: BTreeMap<MempoolCursor, Arc<SeqDbTx>>,
    txs_ordered_by_incremental_id: BTreeMap<SeqDbTxId, Arc<SeqDbTx>>,
    txs_by_hash: HashMap<TxHash, Arc<SeqDbTx>>,
}

impl<Bb: BatchBuilder> Mempool<Bb> {
    /// Creates a new [`Mempool`] with the given capacity and initializes it
    /// with the given transactions.
    pub fn new(
        txsm: TxStatusManager<<<Bb as BatchBuilder>::Spec as Spec>::Da>,
        max_txs_count: NonZero<usize>,
        txs: Vec<SeqDbTx>,
    ) -> anyhow::Result<Self> {
        let mut mempool = Self {
            max_txs_count,
            txsm,
            txs_ordered_by_incremental_id: BTreeMap::new(),
            txs_ordered_by_most_fair_fit: BTreeMap::new(),
            txs_by_hash: HashMap::new(),
        };

        // Initialize the mempool by restoring state.
        for tx in txs.into_iter() {
            mempool.add(Arc::new(tx));
        }

        Ok(mempool)
    }

    pub fn len(&self) -> usize {
        let len = self.txs_by_hash.len();
        assert!(len <= self.max_txs_count.get());

        len
    }

    /// Fetches the next transaction to include in the batch, if a suitable one
    /// exists.
    pub fn next(&self, cursor: &mut MempoolCursor) -> Option<Arc<SeqDbTx>> {
        let mut iter = self
            .txs_ordered_by_most_fair_fit
            // The lower bound is always ignored, so we don't fetch the last
            // transaction again but we go to the next one.
            .range((Bound::Excluded(*cursor), Bound::Unbounded));
        let (next_cursor, tx) = iter.next()?;

        // Important: we update the cursor so the caller can make another call
        // and get the next transaction.
        *cursor = *next_cursor;

        Some(tx.clone())
    }

    /// Remove the tx from the mempool without notifying subscribers
    pub fn remove_without_notifying(&mut self, hash: &TxHash) {
        let Some(tx) = self.txs_by_hash.remove(hash) else {
            return;
        };

        let cursor = MempoolCursor::from_db_tx(&tx);

        self.txs_ordered_by_incremental_id.remove(&tx.uuid_v7);
        self.txs_ordered_by_most_fair_fit.remove(&cursor);
    }

    /// Drop a transaction from the mempool and notify subscribers.
    pub fn drop(&mut self, hash: &TxHash, reason: String) {
        self.remove_without_notifying(hash);
        // Notify about the drop.
        self.txsm.notify(*hash, TxStatus::Dropped { reason });
    }

    fn make_space_for_tx(&mut self) {
        while self.len() >= self.max_txs_count.get() {
            let tx_hash = self
                // We always evict the oldest transaction first.
                .txs_ordered_by_incremental_id
                .first_key_value()
                .expect("Mempool is empty but it doesn't have size zero; this is a bug, please report it")
                .1
                .hash;

            debug!(
                mempool_max_txs_count = self.max_txs_count,
                mempool_current_txs_count = self.len(),
                tx_hash = %HexString::new(tx_hash),
                "Evicting transaction from the mempool to make space for a new one"
            );

            self.drop(&tx_hash, "Mempool is full".to_string());
        }
    }

    pub fn add_new_tx(&mut self, hash: TxHash, baked_tx: FullyBakedTx) -> Arc<SeqDbTx> {
        if let Some(tx) = self.txs_by_hash.get(&hash) {
            // We already have this transaction in the mempool; simply return a
            // reference to it (don't re-add it!).
            tx.clone()
        } else {
            let tx = Arc::new(SeqDbTx::new(hash, baked_tx));
            self.add(tx.clone());
            tx
        }
    }

    pub fn add(&mut self, tx: Arc<SeqDbTx>) {
        self.make_space_for_tx();

        let cursor = MempoolCursor::from_db_tx(&tx);

        self.txs_ordered_by_incremental_id
            .insert(tx.uuid_v7, tx.clone());
        self.txs_ordered_by_most_fair_fit.insert(cursor, tx.clone());
        self.txs_by_hash.insert(tx.hash, tx.clone());
    }
}

/// An opaque cursor for [`FairMempool`] iteration.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
pub struct MempoolCursor {
    tx_size_in_bytes: usize,
    uuid_v7: SeqDbTxId,
}

impl MempoolCursor {
    pub fn new(tx_size_in_bytes: usize) -> Self {
        Self {
            tx_size_in_bytes,
            uuid_v7: 0,
        }
    }

    pub fn from_db_tx(tx: &SeqDbTx) -> Self {
        Self {
            tx_size_in_bytes: tx.tx_bytes.len(),
            uuid_v7: tx.uuid_v7,
        }
    }
}

impl PartialOrd for MempoolCursor {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MempoolCursor {
    fn cmp(&self, other: &Self) -> Ordering {
        // Transactions in the mempool are ordered:
        // 1. from largest to smallest, and
        // 2. by least recent to most recent after that.
        let size_ordering = self.tx_size_in_bytes.cmp(&other.tx_size_in_bytes).reverse();
        let temporal_ordering = self.uuid_v7.cmp(&other.uuid_v7);

        size_ordering.then(temporal_ordering)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_strategy::proptest]
    fn mempool_cursor_ordering_is_correct(
        mc1: MempoolCursor,
        mc2: MempoolCursor,
        mc3: MempoolCursor,
    ) {
        reltester::ord(&mc1, &mc2, &mc3).unwrap();
    }
}
