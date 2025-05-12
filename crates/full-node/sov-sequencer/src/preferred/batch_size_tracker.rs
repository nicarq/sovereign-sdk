pub struct BatchSizeTracker {
    pub max_batch_size: usize,
    pub current_batch_size: usize,
}

impl BatchSizeTracker {
    /// Constant overhead for a serialized batch.
    const BATCH_SIZE_OVERHEAD: usize =
        // 8 bytes for batch_sequence_number
        8
        // 1 byte for visible_slots_to_advance
        + 1
        // 4 bytes for txs vec length
        + 4;

    /// Each transaction is inserted into a vector of transactions in the batch.
    /// BORSH overhead for this is 4 bytes.
    const PER_TX_BORSH_OVERHEAD: usize = 4;

    pub fn new(max_batch_size: usize) -> Self {
        Self {
            max_batch_size,
            current_batch_size: Self::BATCH_SIZE_OVERHEAD,
        }
    }

    pub fn serialized_tx_size(tx_size: usize) -> usize {
        tx_size + Self::PER_TX_BORSH_OVERHEAD
    }

    pub fn can_fit_tx(&self, tx_size: usize) -> bool {
        self.current_batch_size + Self::serialized_tx_size(tx_size) <= self.max_batch_size
    }

    pub fn add_tx(&mut self, tx_size: usize) {
        self.current_batch_size += Self::serialized_tx_size(tx_size);
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZero;

    use sov_blob_storage::PreferredBatchData;
    use sov_modules_api::FullyBakedTx;

    use super::*;

    fn batch_size_calculation_inner(tx_sizes: Vec<usize>) {
        let batch_size = {
            let mut batch = PreferredBatchData {
                sequence_number: 1,
                visible_slots_to_advance: NonZero::new(1).unwrap(),
                data: vec![],
            };

            for tx_size in &tx_sizes {
                batch.data.push(FullyBakedTx::new(vec![0; *tx_size]));
            }

            borsh::to_vec(&batch).unwrap().len()
        };

        let mut tracker = BatchSizeTracker::new(batch_size);

        // The tracker can fit all the transactions...
        for tx_size in tx_sizes {
            assert!(tracker.can_fit_tx(tx_size as _));
            tracker.add_tx(tx_size);
        }

        assert_eq!(tracker.current_batch_size, batch_size);

        // ...and not any others.
        assert!(!tracker.can_fit_tx(1));
    }

    #[test]
    fn batch_size_calculation() {
        batch_size_calculation_inner(vec![]);
        batch_size_calculation_inner(vec![8]);
        batch_size_calculation_inner(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        batch_size_calculation_inner(vec![0]);
    }
}
