#![allow(dead_code)]

use sov_modules_api::capabilities::BlobOrigin;
use sov_modules_api::macros::config_value;
use sov_modules_api::{DaSpec, Spec};

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

    use super::*;
    pub type S = sov_test_utils::TestSpec;

    fn create_blobs(sizes: Vec<usize>) -> Vec<MockBlob> {
        let mut blobs = Vec::new();
        for size in sizes {
            let blob = MockBlob::new(vec![1; size], MockAddress::new([0; 32]), [0; 32]);
            blobs.push(blob);
        }
        blobs
    }

    fn test_helper(sizes: Vec<usize>, expected_sizes: Vec<usize>, max_size: u32) {
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

    #[test]
    fn test_blob_selection() {
        test_helper(vec![], vec![], 0);
        test_helper(vec![], vec![], 5);
        test_helper(vec![5], vec![0], 4);
        test_helper(vec![5], vec![5], 5);
        test_helper(vec![5], vec![5], 10);
        test_helper(vec![20], vec![], 10);
        test_helper(vec![20], vec![20], 20);
        test_helper(vec![20, 30, 10, 40, 19], vec![20, 30, 10], 65);
        test_helper(vec![20, 30, 10, 40, 19], vec![], 0);
    }
}
