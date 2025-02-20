use sov_modules_api::macros::config_value;
use sov_modules_api::{BatchWithId, Spec};
use tracing::error;

use crate::capabilities::BlobDiscardReason;
use crate::{BlobStorage, ValidatedBlob};

struct BlobSizeChecker {
    accumulated_size: u32,
}

impl BlobSizeChecker {
    fn can_process_blob(&self, blob_len: usize, max_size: u32) -> bool {
        // On certain ZKVM platforms, usize is equivalent to u32. While we check for overflows here,
        // it is practically infeasible to encounter batches exceeding 2^32 bytes.

        let Ok(blob_len) = u32::try_from(blob_len) else {
            error!(
                blob_len = %blob_len,
               "BlobSizeChecker: Blob length is, bigger than u32::MAX, this should never happen. The blob was dropped.",
            );

            return false;
        };

        let maybe_accumulated_size = self.accumulated_size.checked_add(blob_len);

        match maybe_accumulated_size {
            Some(new_accumulated_size) if new_accumulated_size <= max_size => true,
            _ => {
                // The full-node/prover is capable of processing batches of up to hundreds of megabytes in size.
                // However, the slot limits on the DAs are in the range of single megabytes, making it impossible to reach these limits.
                // These checks are added as a precaution, but such a scenario should not occur.
                error!(
                    max_size = max_size,
                    blob_len = %blob_len,
                    accumulated_size = %self.accumulated_size,
                   "BlobSizeChecker: Unable to accumulate the blob, this should never happen.",
                );

                false
            }
        }
    }
}

/// This component limits the data returned from the `BlobSelector` to the `STF` blueprint.
/// While the `STF` blueprint can handle data sizes in the range of hundreds of megabytes, the DA layer block sizes are typically only a few megabytes.
/// As a result, this mechanism acts as a precautionary sanity check.
/// Example
/// If the maximum allowed size is 10MB, and the blobs are inserted in the following order [3MB, 20MB, 1MB], the `BlobsWithTotalSizeLimit` will only hold the first and the last blobs because their total size is less than 10MB.
pub(crate) struct BlobsWithTotalSizeLimit<S: Spec> {
    blobs_with_address: Vec<ValidatedBlob<S, BatchWithId<S>>>,
    blob_size_checker: BlobSizeChecker,
    max_total_size: u32,
    // `max_total_size` is always greater than `max_preferred_blob_size`,
    // preventing a single preferred blob from consuming all available space to censor standard sequencers.
    // Currently, the DAs we support allow blobs in the MB range, with `max_total_size` being an order of magnitude larger.
    // While censorship is not possible under current conditions, we include this logic as a precautionary measure.
    max_preffered_blob_size: u32,
}

impl<S: Spec> BlobsWithTotalSizeLimit<S> {
    pub(crate) fn new() -> Self {
        Self::new_with_size(config_value!(
            "MAX_ALLOWED_DATA_SIZE_RETURNED_BY_BLOB_STORAGE"
        ))
    }

    fn new_with_size(max_total_size: u32) -> Self {
        let max_preffered_blob_size = max_total_size
            .checked_div(sov_modules_api::PREFFERERD_DATA_FRACTION.denominator)
            .expect("Can't devide by 0")
            .checked_mul(sov_modules_api::PREFFERERD_DATA_FRACTION.numerator)
            // This cannot overflow because the PREFFERERD_DATA_FRACTION must be less than 1.
            .unwrap();

        Self {
            blobs_with_address: Vec::new(),
            blob_size_checker: BlobSizeChecker {
                accumulated_size: 0,
            },
            max_total_size,
            max_preffered_blob_size,
        }
    }

    pub(crate) fn push_preffered_or_ignore(
        &mut self,
        elem: ValidatedBlob<S, BatchWithId<S>>,
    ) -> bool {
        let can_process_blob = self
            .blob_size_checker
            .can_process_blob(elem.blob.blob_size(), self.max_preffered_blob_size);

        if can_process_blob {
            // SAFETY: we checked that the blob size checker has capacity for this blob just above, which ensures that this addition will not overflow.
            self.blob_size_checker
                .accumulated_size
                .checked_add(elem.blob.blob_size() as u32)
                .unwrap();
            self.blobs_with_address.push(elem);
        }

        can_process_blob
    }

    /// Returns true if the blob can be accepted.
    pub(crate) fn can_accept_blob(&self, blob_len: usize) -> bool {
        self.blob_size_checker
            .can_process_blob(blob_len, self.max_total_size)
    }

    /// Stores the blob internally if the total size of stored blobs didn't corss the preconfigued limit.
    pub(crate) fn push_or_ignore(&mut self, elem: ValidatedBlob<S, BatchWithId<S>>) -> bool {
        let can_process_blob = self
            .blob_size_checker
            .can_process_blob(elem.blob.blob_size(), self.max_total_size);

        if can_process_blob {
            // SAFETY: We just checked that the blob size checker has capacity for this blob so this computation will not overflow.
            self.blob_size_checker.accumulated_size += elem.blob.blob_size() as u32;
            self.blobs_with_address.push(elem);
        } else {
            BlobStorage::<S>::log_discarded_item(
                &elem.sender,
                elem.blob.id(),
                &BlobDiscardReason::OutOfCapacity,
            );
        }

        can_process_blob
    }

    pub(crate) fn inner(self) -> Vec<ValidatedBlob<S, BatchWithId<S>>> {
        self.blobs_with_address
    }
}

#[cfg(test)]
mod tests {
    use sov_modules_api::{BatchWithId, BlobDataWithId, FullyBakedTx};

    use super::*;
    use crate::Escrow;
    pub type S = sov_test_utils::TestSpec;

    #[test]
    fn test_blob_outputs() {
        test_helper_correct_blob_selection_outputs(vec![1, 2, 3], vec![0, 1, 2], 10);
        test_helper_correct_blob_selection_outputs(vec![1, 14, 11], vec![0], 10);
        test_helper_correct_blob_selection_outputs(vec![11, 2, 3], vec![1, 2], 10);
        test_helper_correct_blob_selection_outputs(vec![10, 2, 3], vec![0], 10);
        test_helper_correct_blob_selection_outputs(vec![3, 22, 1, 88, 7], vec![0, 2], 10);
        test_helper_correct_blob_selection_outputs(vec![10], vec![0], 10);
        test_helper_correct_blob_selection_outputs(vec![0], vec![0], 10);
    }

    fn make_tx_batches_of_given_size(sizes: Vec<usize>) -> Vec<Vec<FullyBakedTx>> {
        let mut batches_of_txs = Vec::new();

        for size in sizes {
            let tx = FullyBakedTx::new(vec![0; size]);
            batches_of_txs.push(vec![tx]);
        }

        batches_of_txs
    }

    fn test_helper_correct_blob_selection_outputs(
        blob_sizes: Vec<usize>,
        expected_indexes: Vec<u8>,
        max_size: u32,
    ) {
        let mut blobs_with_total_size_limit = BlobsWithTotalSizeLimit::<S>::new_with_size(max_size);
        let txs = make_tx_batches_of_given_size(blob_sizes);

        let mut expected_addresses = Vec::new();
        for (i, b) in txs.into_iter().enumerate() {
            let addr = [i as u8; 32].into();
            let b = ValidatedBlob::new(
                BlobDataWithId::Batch(BatchWithId::new(b, [0; 32], [i as u8; 28].into())),
                addr,
                Escrow::None,
            );
            if expected_indexes.contains(&(i as u8)) {
                expected_addresses.push(addr);
            }
            blobs_with_total_size_limit.push_or_ignore(b);
        }

        let inner = blobs_with_total_size_limit
            .inner()
            .into_iter()
            .map(|b| b.sender)
            .collect::<Vec<_>>();

        assert_eq!(inner, expected_addresses);
    }

    fn create_blob(size: usize) -> ValidatedBlob<S, BatchWithId<S>> {
        ValidatedBlob::new(
            BlobDataWithId::Batch(BatchWithId::new(
                vec![FullyBakedTx::new(vec![0; size])],
                [0; 32],
                [0; 28].into(),
            )),
            [0; 32].into(),
            Escrow::None,
        )
    }

    #[test]
    fn test_preffered_blob_outputs() {
        let max_size = 100;

        let mut blobs_with_total_size_limit = BlobsWithTotalSizeLimit::<S>::new_with_size(max_size);

        let blob_size = 95;
        let blob = create_blob(blob_size);

        // The blob is too large to be processed as a preferred blob.
        {
            let can_process_blob =
                blobs_with_total_size_limit.push_preffered_or_ignore(blob.clone());
            assert!(!can_process_blob);
        }

        // The blob can be processed as a regular blob.
        {
            let can_process_blob = blobs_with_total_size_limit.push_or_ignore(blob);
            assert!(can_process_blob);
        }
    }

    #[test]
    fn test_preffered_and_standard_blob() {
        let max_size = 100;

        let mut blobs_with_total_size_limit = BlobsWithTotalSizeLimit::<S>::new_with_size(max_size);

        let blob_size = 80;
        let blob = create_blob(blob_size);

        {
            let can_process_blob = blobs_with_total_size_limit.push_preffered_or_ignore(blob);
            assert!(can_process_blob);
        }

        let blob_size = 10;
        let blob = create_blob(blob_size);

        {
            let can_process_blob = blobs_with_total_size_limit.push_or_ignore(blob);
            assert!(can_process_blob);
        }
    }
}
