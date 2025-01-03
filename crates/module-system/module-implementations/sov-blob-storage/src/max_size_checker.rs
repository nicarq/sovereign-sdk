#![allow(dead_code)]

use sov_modules_api::capabilities::BlobOrigin;
use sov_modules_api::macros::config_value;
use sov_modules_api::{BatchWithId, BlobDataWithId, DaSpec, Spec};
use tracing::error;

use crate::{BlobAndSender, SequencerType};

struct BlobSizeChecker {
    max_size: u32,
    accumulated_size: u32,
}

impl BlobSizeChecker {
    fn can_process_blob(&mut self, blob_len: usize) -> bool {
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
            Some(new_accumulated_size) if new_accumulated_size <= self.max_size => {
                self.accumulated_size = new_accumulated_size;
                true
            }
            _ => {
                // The full-node/prover is capable of processing batches of up to hundreds of megabytes in size.
                // However, the slot limits on the DAs are in the range of single megabytes, making it impossible to reach these limits.
                // These checks are added as a precaution, but such a scenario should not occur.
                error!(
                    max_size = self.max_size,
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
    blobs_with_address: Vec<(BlobDataWithId<BatchWithId>, SequencerType<S>)>,
    blob_size_checker: BlobSizeChecker,
}

impl<S: Spec> BlobsWithTotalSizeLimit<S> {
    pub(crate) fn new() -> Self {
        Self::new_with_size(config_value!(
            "MAX_ALLOWED_DATA_SIZE_RETURNED_BY_BLOB_STORAGE"
        ))
    }

    fn new_with_size(max_size: u32) -> Self {
        Self {
            blobs_with_address: Vec::new(),
            blob_size_checker: BlobSizeChecker {
                max_size,
                accumulated_size: 0,
            },
        }
    }

    /// Stores the blob internally if the total size of stored blobs didn't corss the preconfigued limit.
    pub(crate) fn push_or_ignore(&mut self, elem: BlobAndSender<S>) -> bool {
        let can_process_blob = self.blob_size_checker.can_process_blob(elem.0.blob_size());

        if can_process_blob {
            self.blobs_with_address.push(elem);
        }

        can_process_blob
    }

    pub(crate) fn inner(self) -> Vec<BlobAndSender<S>> {
        self.blobs_with_address
    }
}

/// Selects blobs until their total accumulated size surpasses [`MAX_ALLOWED_SLOT_SIZE_IN_BLOB_STORAGE`].
/// If the first blob exceeds [`MAX_ALLOWED_SLOT_SIZE_IN_BLOB_STORAGE`], this function will not return any data,
/// even if subsequent blobs are smaller.
/// Note: In practice, this limit is significantly larger than the typical limit of block size for most of the DA layers,
/// serving primarily as a sanity check.
pub(crate) fn take_blobs_with_size_limit<'a, I, S: Spec>(
    current_blobs: I,
) -> impl Iterator<Item = BlobOrigin<'a, <S::Da as DaSpec>::BlobTransaction>>
where
    I: IntoIterator<Item = BlobOrigin<'a, <S::Da as DaSpec>::BlobTransaction>>,
{
    take_blobs_with_size_limit_inner::<_, S>(
        current_blobs,
        config_value!("MAX_ALLOWED_SLOT_SIZE_IN_BLOB_STORAGE"),
    )
}

fn take_blobs_with_size_limit_inner<'a, I, S: Spec>(
    current_blobs: I,
    max_size: u32,
) -> impl Iterator<Item = BlobOrigin<'a, <S::Da as DaSpec>::BlobTransaction>>
where
    I: IntoIterator<Item = BlobOrigin<'a, <S::Da as DaSpec>::BlobTransaction>>,
{
    current_blobs
        .into_iter()
        .scan(0, move |accumulated_size: &mut u32, elem| {
            // On certain ZKVM platforms, usize is equivalent to u32. While we check for overflows here,
            // it is practically infeasible to encounter batches exceeding 2^32 bytes.
            let blob_len = u32::try_from(elem.total_len());

            let Ok(blob_len) = blob_len else {
                return None;
            };

            let maybe_accumulated_size = accumulated_size.checked_add(blob_len);

            match maybe_accumulated_size {
                Some(new_accumulated_size) if new_accumulated_size <= max_size => {
                    *accumulated_size = new_accumulated_size;
                    Some(elem)
                }
                _ => None,
            }
        })
}

#[cfg(test)]
mod tests {
    use sov_mock_da::{MockAddress, MockBlob};
    use sov_modules_api::FullyBakedTx;

    use super::*;
    pub type S = sov_test_utils::TestSpec;

    #[test]
    fn test_blob_size_selection() {
        test_helper_blob_size_limit(vec![], vec![], 0);
        test_helper_blob_size_limit(vec![], vec![], 5);
        test_helper_blob_size_limit(vec![5], vec![0], 4);
        test_helper_blob_size_limit(vec![5], vec![5], 5);
        test_helper_blob_size_limit(vec![5], vec![5], 10);
        test_helper_blob_size_limit(vec![20], vec![], 10);
        test_helper_blob_size_limit(vec![20], vec![20], 20);
        test_helper_blob_size_limit(vec![20, 30, 10, 40, 19], vec![20, 30, 10], 65);
        test_helper_blob_size_limit(vec![20, 30, 10, 40, 19], vec![], 0);
    }

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

    fn create_blobs(sizes: Vec<usize>) -> Vec<MockBlob> {
        let mut blobs = Vec::new();
        for size in sizes {
            let blob = MockBlob::new(vec![1; size], MockAddress::new([0; 32]), [0; 32]);
            blobs.push(blob);
        }
        blobs
    }

    fn test_helper_blob_size_limit(sizes: Vec<usize>, expected_sizes: Vec<usize>, max_size: u32) {
        let mut blobs = create_blobs(sizes);

        let blobs_origins = blobs.iter_mut().map(BlobOrigin::Batch).collect::<Vec<_>>();

        let accumulated_blobs =
            take_blobs_with_size_limit_inner::<_, S>(blobs_origins, max_size).collect::<Vec<_>>();

        for (i, b) in accumulated_blobs.iter().enumerate() {
            assert_eq!(b.total_len(), expected_sizes[i]);
        }

        let total_accumulated_size = accumulated_blobs
            .iter()
            .fold(0, |acc, blob| acc + blob.total_len());

        assert!(
            total_accumulated_size <= (max_size as usize),
            "The total size of the blobs should be less than or equal to the max size"
        );
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
            let b = BlobDataWithId::Batch(BatchWithId::new(b, [0; 32]));
            let addr = MockAddress::new([i as u8; 32]);
            if expected_indexes.contains(&(i as u8)) {
                expected_addresses.push(SequencerType::Standard(addr));
            }
            blobs_with_total_size_limit.push_or_ignore((b, SequencerType::Standard(addr)));
        }

        let inner = blobs_with_total_size_limit
            .inner()
            .into_iter()
            .map(|(_, addr)| addr)
            .collect::<Vec<_>>();

        assert_eq!(inner, expected_addresses);
    }
}
