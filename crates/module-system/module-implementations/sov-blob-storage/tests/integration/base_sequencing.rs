use sov_blob_storage::BlobStorage;

use crate::helpers_basic_kernel::{
    assert_blobs_are_correctly_received_basic_kernel, setup_basic_kernel,
};
use crate::{TestData, S};

#[test]
fn empty_test() {
    let (_, runner) = setup_basic_kernel();

    runner.query_visible_state(|state| {
        assert!(BlobStorage::<S>::default()
            .take_blobs_for_rollup_height(1, state)
            .is_empty());
    });
}

/// Tests that the blob storage module can store and retrieve blobs.
/// The test creates a batch of blobs, and then checks that the blobs are stored and retrieved correctly by
/// comparing the hashes of the blobs sent and the hashes of the receipts received.
#[test]
fn store_and_retrieve_standard_basic_kernel() {
    let (
        TestData {
            preferred_sequencer,
            ..
        },
        mut runner,
    ) = setup_basic_kernel();

    runner.query_visible_state(|state| {
        let blob_storage = BlobStorage::<S>::default();

        assert!(blob_storage
            .take_blobs_for_rollup_height(1, state)
            .is_empty());
        assert!(blob_storage
            .take_blobs_for_rollup_height(2, state)
            .is_empty());
        assert!(blob_storage
            .take_blobs_for_rollup_height(3, state)
            .is_empty());
        assert!(blob_storage
            .take_blobs_for_rollup_height(4, state)
            .is_empty());
    });

    runner.advance_slots(1);

    // Create three slots, each containing a batch of blobs.
    // We should receive three receipts in the same order as the blobs were sent.
    let slots = vec![
        vec![preferred_sequencer.clone(); 3],
        vec![preferred_sequencer.clone()],
        vec![preferred_sequencer.clone()],
    ];

    assert_blobs_are_correctly_received_basic_kernel(
        slots,
        vec![vec![0, 1, 2], vec![3], vec![4]],
        vec![1, 1, 1],
        &mut runner,
    );
}
