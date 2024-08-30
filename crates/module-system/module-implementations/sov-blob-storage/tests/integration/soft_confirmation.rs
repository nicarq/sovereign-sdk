use sov_blob_storage::{BlobStorage, DEFERRED_SLOTS_COUNT};
use sov_test_utils::SequencerInfo;

use crate::helpers_soft_confirmations::{
    assert_blobs_are_correctly_received_soft_confirmation, setup_soft_confirmation_kernel,
    setup_with_registration_soft_confirmation_kernel,
};
use crate::{Da, TestData, S};

/// Tests the soft confirmation kernel functionality by executing one batch per slot for the preferred sequencer.
/// Expected result:
/// - Slot 1: Send [Batch 1]. Receive [Batch 1]
/// - Slot 2: Send [Batch 2]. Receive [Batch 2]
/// - Slot 3: Send [Batch 3]. Receive [Batch 3]
#[test]
fn store_and_retrieve_standard_soft_confirmation_kernel() {
    let (
        TestData {
            preferred_sequencer,
            ..
        },
        mut runner,
    ) = setup_soft_confirmation_kernel();

    runner.query_state(|state| {
        let blob_storage = BlobStorage::<S, Da>::default();

        assert!(blob_storage.take_blobs_for_slot_number(1, state).is_empty());
        assert!(blob_storage.take_blobs_for_slot_number(2, state).is_empty());
        assert!(blob_storage.take_blobs_for_slot_number(3, state).is_empty());
        assert!(blob_storage.take_blobs_for_slot_number(4, state).is_empty());
    });

    runner.advance_slots(1);

    // Create three slots, each containing a batch of blobs.
    // We should receive three receipts in the same order as the blobs were sent.
    let slots = vec![
        vec![(
            preferred_sequencer.clone(),
            SequencerInfo::Preferred {
                slots_to_advance: 1,
                sequence_number: 0,
            },
        )],
        vec![(
            preferred_sequencer.clone(),
            SequencerInfo::Preferred {
                slots_to_advance: 1,
                sequence_number: 1,
            },
        )],
        vec![(
            preferred_sequencer.clone(),
            SequencerInfo::Preferred {
                slots_to_advance: 1,
                sequence_number: 2,
            },
        )],
    ];

    assert_blobs_are_correctly_received_soft_confirmation(
        slots,
        vec![vec![0], vec![1], vec![2]],
        &mut runner,
    );
}

/// Create three slots using the preferred sequencer.
/// The first slot contains three batches, the next two slots contain one batch each.
/// We should receive the receipts in the following order:
/// Slot 1: Send [Batch 0, Batch 1, Batch 2]. Receive [Batch 0]
/// Slot 2: Send [Batch 3]. Receive [Batch 1]
/// Slot 3: Send [Batch 4]. Receive [Batch 2]
/// Slot 4: Send []. Receive [Batch 3]
/// Slot 5: Send []. Receive [Batch 4]
#[test]
fn store_and_retrieve_standard_soft_confirmation_kernel_deferred() {
    let (
        TestData {
            preferred_sequencer,
            ..
        },
        mut runner,
    ) = setup_soft_confirmation_kernel();

    let slots = vec![
        vec![
            (
                preferred_sequencer.clone(),
                SequencerInfo::Preferred {
                    slots_to_advance: 1,
                    sequence_number: 0,
                },
            ),
            (
                preferred_sequencer.clone(),
                SequencerInfo::Preferred {
                    slots_to_advance: 1,
                    sequence_number: 1,
                },
            ),
            (
                preferred_sequencer.clone(),
                SequencerInfo::Preferred {
                    slots_to_advance: 1,
                    sequence_number: 2,
                },
            ),
        ],
        vec![(
            preferred_sequencer.clone(),
            SequencerInfo::Preferred {
                slots_to_advance: 1,
                sequence_number: 3,
            },
        )],
        vec![(
            preferred_sequencer.clone(),
            SequencerInfo::Preferred {
                slots_to_advance: 1,
                sequence_number: 4,
            },
        )],
    ];

    assert_blobs_are_correctly_received_soft_confirmation(
        slots,
        vec![vec![0], vec![1], vec![2], vec![3], vec![4]],
        &mut runner,
    );
}

/// Tests that the blobs from the non-preferred sequencer are deferred.
/// Send a batch from the non-preferred sequencer. We should have the following receipts:
/// Slot 1: Send [Batch 0]. Receive []
/// Slots [1..`DEFERRED_SLOTS_COUNT`-1]: Send []. Receive []
/// Slot [`DEFERRED_SLOTS_COUNT`]: Send []. Receive [Batch 1]
#[test]
fn non_preferred_sequencer_deferred() {
    let (
        TestData {
            regular_sequencer, ..
        },
        mut runner,
    ) = setup_with_registration_soft_confirmation_kernel();

    let slots = vec![vec![(regular_sequencer.clone(), SequencerInfo::Regular)]];

    let mut receive_order = vec![vec![]; DEFERRED_SLOTS_COUNT as usize];
    receive_order.push(vec![0]);

    assert_blobs_are_correctly_received_soft_confirmation(slots, receive_order, &mut runner);
}

/// Interspace slots between the preferred and the non preferred sequencer. Assuming we have a [`DEFERRED_SLOTS_COUNT`] == 2,
/// We simulate the following scenario:
/// - Slot 1: Send [(Pref, Blob 1), (Standard, Blob 2), (Pref, Blob 3)] | Recv [(Pref, Blob 1)]
/// - Slot 2: Send [(Pref, Blob 4)], (Standard, Blob 5), (Standard, Blob 6)] | Recv [(Pref, Blob 3)]
/// - Slot 3: Send [(Pref, Blob 7)] | Recv [(Pref, Blob 4)]
/// - Slot 5: Send [] | Recv [(Standard, Blob 2), (Pref, Blob 7)]
/// - Slot 6: Send [] | Recv [(Standard, Blob 5), (Standard, Blob 6)]
#[test]
fn interspace_slots_preferred_non_preferred_sequencer() {}
