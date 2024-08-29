use std::collections::HashMap;

use sov_blob_storage::BlobStorage;
use sov_mock_da::{MockBlob, MockDaSpec, MockHash};
use sov_modules_api::{BlobReaderTrait, DaSpec};
use sov_rollup_interface::da::RelevantBlobs;
use sov_test_utils::runtime::ValueSetter;

use crate::{build_blobs, setup, TestData};

type S = sov_test_utils::TestSpec;
type Da = MockDaSpec;

#[test]
fn empty_test() {
    let (_, runner) = setup();

    runner.query_state(|state| {
        assert!(BlobStorage::<S, Da>::default()
            .take_blobs_for_slot_number(1, state)
            .is_empty());
    });
}

fn check_received_blobs(
    sent_blobs: Vec<MockBlob>,
    received_batch_hashes_and_sender: Vec<([u8; 32], <MockDaSpec as DaSpec>::Address)>,
) {
    assert_eq!(sent_blobs.len(), received_batch_hashes_and_sender.len());

    sent_blobs
        .into_iter()
        .zip(received_batch_hashes_and_sender)
        .for_each(
            |(mock_blob, (received_batch_id, received_sender_address))| {
                assert_eq!(mock_blob.hash(), MockHash(received_batch_id));
                assert_eq!(mock_blob.sender(), received_sender_address);
            },
        );
}

/// Tests that the blob storage module can store and retrieve blobs.
/// The test creates a batch of blobs, and then checks that the blobs are stored and retrieved correctly by
/// comparing the hashes of the blobs sent and the hashes of the receipts received.
#[test]
fn store_and_retrieve_standard() {
    let (
        TestData {
            user,
            preferred_sequencer,
            ..
        },
        mut runner,
    ) = setup();

    runner.query_state(|state| {
        let blob_storage = BlobStorage::<S, Da>::default();

        assert!(blob_storage.take_blobs_for_slot_number(1, state).is_empty());
        assert!(blob_storage.take_blobs_for_slot_number(2, state).is_empty());
        assert!(blob_storage.take_blobs_for_slot_number(3, state).is_empty());
        assert!(blob_storage.take_blobs_for_slot_number(4, state).is_empty());
    });

    runner.advance_slots(1);

    let mut nonces = HashMap::new();

    // Create three slots, each containing a batch of blobs.
    // We should receive three receipts in the same order as the blobs were sent.
    let slots = vec![
        build_blobs(
            &user,
            vec![vec![preferred_sequencer.clone(); 3]],
            &mut nonces,
            &mut runner,
        ),
        build_blobs(
            &user,
            vec![vec![preferred_sequencer.clone()]],
            &mut nonces,
            &mut runner,
        ),
        build_blobs(
            &user,
            vec![vec![preferred_sequencer.clone()]],
            &mut nonces,
            &mut runner,
        ),
    ];

    let mut batch_ids_and_sender = Vec::new();
    for slot in slots.clone() {
        let result = runner.execute::<RelevantBlobs<MockBlob>, ValueSetter<S>>(slot, None);

        batch_ids_and_sender.push(
            result
                .batch_receipts
                .iter()
                .map(|b| (b.batch_hash, preferred_sequencer.da_address))
                .collect::<Vec<_>>(),
        );
    }

    check_received_blobs(
        slots[0].clone().batch_blobs,
        batch_ids_and_sender[0].clone(),
    );
    check_received_blobs(
        slots[1].clone().batch_blobs,
        batch_ids_and_sender[1].clone(),
    );
    check_received_blobs(
        slots[2].clone().batch_blobs,
        batch_ids_and_sender[2].clone(),
    );
}
