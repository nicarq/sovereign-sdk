use sov_blob_storage::{config_deferred_slots_count, BlobStorage};
use sov_test_utils::SequencerInfo;

use crate::helpers_soft_confirmations::{
    assert_blobs_are_correctly_received_soft_confirmation, setup_soft_confirmation_kernel,
    setup_with_registration_soft_confirmation_kernel,
};
use crate::{Da, TestData, S};

/// Tests the soft confirmation kernel functionality by executing one batch per slot for the preferred sequencer.
/// Expected result:
/// - Slot 1: Send []. Receive []
/// - Slot 2: Send [Batch 0]. Receive [Batch 0]
/// - Slot 3: Send [Batch 1]. Receive [Batch 1]
/// - Slot 4: Send [Batch 2]. Receive [Batch 2]
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
        vec![1, 1, 1],
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
        vec![1, 1, 1, 1, 1],
        &mut runner,
    );
}

/// Tests that the blobs from the non-preferred sequencer are deferred.
/// Send a batch from the non-preferred sequencer. We should have the following receipts:
/// Slot 1: Send [Batch 0]. Receive []
/// Slots [1..`DEFERRED_SLOTS_COUNT`-1]: Send []. Receive []
/// Slot [`DEFERRED_SLOTS_COUNT`]: Send []. Receive [Batch 0]
#[test]
fn non_preferred_sequencer_deferred() {
    let (
        TestData {
            regular_sequencer, ..
        },
        mut runner,
    ) = setup_with_registration_soft_confirmation_kernel();

    let slots = vec![vec![(regular_sequencer.clone(), SequencerInfo::Regular)]];

    let mut receive_order = vec![vec![]; (config_deferred_slots_count() - 1) as usize];
    receive_order.push(vec![0]);

    let mut virtual_slot_heights = vec![0; (config_deferred_slots_count() - 1) as usize];

    virtual_slot_heights.push(1);

    assert_blobs_are_correctly_received_soft_confirmation(
        slots,
        receive_order,
        virtual_slot_heights,
        &mut runner,
    );
}

/// Interspace slots between the preferred and the non preferred sequencer. We advance the slots every time with preferred sequencer transactions.
/// Assuming we have a [`DEFERRED_SLOTS_COUNT`] == 5,
/// We simulate the following scenario:
/// - Slot 1: Send [(Pref, Blob 0), (Standard, Blob 1), (Pref, Blob 2) | Recv [Blob 0, Blob 1]
/// - Slot 2: Send [(Pref, Blob 3)], (Standard, Blob 4), (Standard, Blob 5)] | Recv [Blob 2, Blob 4, Blob 5]
/// - Slot 3: Send [(Pref, Blob 6)] | Recv [Blob 3]
/// - Slot 5: Send [] | Recv [Blob 6]
#[test]
fn interspace_slots_preferred_non_preferred_sequencer_increase_slots() {
    let (
        TestData {
            preferred_sequencer,
            regular_sequencer,
            ..
        },
        mut runner,
    ) = setup_with_registration_soft_confirmation_kernel();

    let slots = vec![
        vec![
            (
                preferred_sequencer.clone(),
                SequencerInfo::Preferred {
                    slots_to_advance: 1,
                    sequence_number: 1,
                },
            ),
            (regular_sequencer.clone(), SequencerInfo::Regular),
            (
                preferred_sequencer.clone(),
                SequencerInfo::Preferred {
                    slots_to_advance: 1,
                    sequence_number: 2,
                },
            ),
        ],
        vec![
            (
                preferred_sequencer.clone(),
                SequencerInfo::Preferred {
                    slots_to_advance: 1,
                    sequence_number: 3,
                },
            ),
            (regular_sequencer.clone(), SequencerInfo::Regular),
            (regular_sequencer.clone(), SequencerInfo::Regular),
        ],
        vec![(
            preferred_sequencer.clone(),
            SequencerInfo::Preferred {
                slots_to_advance: 1,
                sequence_number: 4,
            },
        )],
    ];

    let receive_order = vec![vec![0, 1], vec![2, 4, 5], vec![3], vec![6]];

    let virtual_slot_heights_increases = vec![1, 1, 1, 1];

    assert_blobs_are_correctly_received_soft_confirmation(
        slots,
        receive_order,
        virtual_slot_heights_increases,
        &mut runner,
    );
}

/// Interspace slots between the preferred and the non preferred sequencer. We don't advance the slots every time with preferred sequencer transactions.
/// Assuming we have a [`DEFERRED_SLOTS_COUNT`] == 5,
/// We simulate the following scenario:
/// - Slot 1: Send [(Pref, Blob 0), (Standard, Blob 1), (Pref, Blob 2) | Recv [Blob 0, Blob 1]
/// - Slot 2: Send [(Standard, Blob 3)], (Standard, Blob 4), (Standard, Blob 5)] | Recv [Blob 2, Blob 3, Blob 4, Blob 5]
/// - Slot 3: Send [(Standard, Blob 6)] | Recv []
/// - Slot 4: Send [] | Recv []
/// - Slot 5: Send [] | Recv []
/// - Slot 6: Send [] | Recv []
/// - Slot 7: Send [] | Recv [Blob 6]
#[test]
fn interspace_slots_preferred_non_preferred_sequencer_dont_advance_slots() {
    let (
        TestData {
            preferred_sequencer,
            regular_sequencer,
            ..
        },
        mut runner,
    ) = setup_with_registration_soft_confirmation_kernel();

    let slots = vec![
        vec![
            (
                preferred_sequencer.clone(),
                SequencerInfo::Preferred {
                    slots_to_advance: 1,
                    sequence_number: 1,
                },
            ),
            (regular_sequencer.clone(), SequencerInfo::Regular),
            (
                preferred_sequencer.clone(),
                SequencerInfo::Preferred {
                    slots_to_advance: 1,
                    sequence_number: 2,
                },
            ),
        ],
        vec![
            (regular_sequencer.clone(), SequencerInfo::Regular),
            (regular_sequencer.clone(), SequencerInfo::Regular),
            (regular_sequencer.clone(), SequencerInfo::Regular),
        ],
        vec![(regular_sequencer.clone(), SequencerInfo::Regular)],
    ];

    let mut receive_order = vec![vec![0, 1], vec![2, 3, 4, 5]];
    receive_order.append(&mut vec![
        vec![];
        (config_deferred_slots_count() - 1) as usize
    ]);
    receive_order.push(vec![6]);

    let mut virtual_slot_heights_increases = vec![1, 1];
    virtual_slot_heights_increases
        .append(&mut vec![0; (config_deferred_slots_count() - 1) as usize]);
    virtual_slot_heights_increases.push(1);

    assert_blobs_are_correctly_received_soft_confirmation(
        slots,
        receive_order,
        virtual_slot_heights_increases,
        &mut runner,
    );
}

/// Test that the preferred sequencer is able to force execute blobs
/// We test the following scenario:
/// - Slot 1: Send [(Batch 0, Regular)]. Receive []
/// - Slot 2: Send [(Batch 1, Regular)]. Receive []
/// - Slot 3: Send [(Batch 2, Regular)]. Receive []
/// - Slot 4: Send [(Batch 3, Preferred, Height increase 2). Receive [Batch 3, Batch 0, Batch 1]]
///
/// This test assumes that [`DEFERRED_SLOTS_COUNT`] is greater than 2.
#[test]
fn send_slots_with_high_deferred_slot_adjustment() {
    let (
        TestData {
            preferred_sequencer,
            regular_sequencer,
            ..
        },
        mut runner,
    ) = setup_with_registration_soft_confirmation_kernel();

    let slots_info = vec![
        vec![(regular_sequencer.clone(), SequencerInfo::Regular)],
        vec![(regular_sequencer.clone(), SequencerInfo::Regular)],
        vec![(regular_sequencer.clone(), SequencerInfo::Regular)],
        vec![(
            preferred_sequencer.clone(),
            SequencerInfo::Preferred {
                slots_to_advance: 2,
                sequence_number: 1,
            },
        )],
    ];

    let receive_order = vec![vec![], vec![], vec![], vec![3, 0, 1]];

    let virtual_slot_heights_increases = vec![0, 0, 0, 2];

    assert_blobs_are_correctly_received_soft_confirmation(
        slots_info,
        receive_order,
        virtual_slot_heights_increases,
        &mut runner,
    );
}

/// When sending a blob with an outdated sequencer number, the blob should be dropped.
#[test]
fn blobs_with_low_sequencer_number_get_dropped() {
    let (
        TestData {
            preferred_sequencer,
            ..
        },
        mut runner,
    ) = setup_soft_confirmation_kernel();

    let slots = vec![
        vec![(
            preferred_sequencer.clone(),
            SequencerInfo::Preferred {
                slots_to_advance: 1,
                sequence_number: 0,
            },
        )],
        // This blob should be dropped because, after the blob above, the sequence number becomes 2.
        vec![(
            preferred_sequencer.clone(),
            SequencerInfo::Preferred {
                slots_to_advance: 1,
                sequence_number: 0,
            },
        )],
    ];

    let receive_order = vec![vec![0], vec![]];

    assert_blobs_are_correctly_received_soft_confirmation(
        slots,
        receive_order,
        vec![1, 0],
        &mut runner,
    );
}

/// Check that blobs with higher sequencer numbers get deferred.
/// We test the following situation:
/// - Slot 1: Send [(Batch 0, Priority, Sequencer number 2)]. Receive []
/// - Slot 2: Send [(Batch 1, Priority, Sequencer number 1)]. Receive []
/// - Slot 3: Send [(Batch 2, Priority, Sequencer number 0)]. Receive [Batch 2]
/// - Slot 4: Send []. Receive [Batch 1]
/// - Slot 5: Send []. Receive [Batch 0]
#[test]
fn blobs_with_high_sequencer_number_get_deferred() {
    let (
        TestData {
            preferred_sequencer,
            ..
        },
        mut runner,
    ) = setup_soft_confirmation_kernel();

    let slots = vec![
        vec![(
            preferred_sequencer.clone(),
            SequencerInfo::Preferred {
                slots_to_advance: 1,
                sequence_number: 2,
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
                sequence_number: 0,
            },
        )],
    ];

    let receive_order = vec![vec![], vec![], vec![2], vec![1], vec![0]];

    assert_blobs_are_correctly_received_soft_confirmation(
        slots,
        receive_order,
        vec![0, 0, 1, 1, 1],
        &mut runner,
    );
}
