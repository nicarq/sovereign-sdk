use std::collections::HashMap;

use sov_mock_da::{MockBlob, MockDaSpec, MockHash};
use sov_modules_api::capabilities::{BlobSelector, KernelSlotHooks};
use sov_modules_api::{BlobDataWithId, BlobReaderTrait, DaSpec, Spec};
use sov_modules_stf_blueprint::BatchReceipt;
use sov_rollup_interface::da::RelevantBlobs;
use sov_test_utils::runtime::{SlotReceipt, TestRunnerWithKernel, ValueSetter};
use sov_test_utils::{generate_optimistic_runtime, TestSequencer, TestUser};

mod capability_tests;

mod helpers_basic_kernel;
mod helpers_soft_confirmations;

mod basic_kernel;
mod soft_confirmation;

pub type S = sov_test_utils::TestSpec;
pub type Da = MockDaSpec;

pub type SlotConfigInfo<SequencerInfo> = Vec<SequencerInfo>;

pub struct TestData<S: Spec> {
    pub user: TestUser<S>,
    pub preferred_sequencer: TestSequencer<S, MockDaSpec>,
    pub regular_sequencer: TestSequencer<S, MockDaSpec>,
}

pub type TestRunner<K> = TestRunnerWithKernel<RT, K, S>;
pub type RT = TestBlobStorageRuntime<S, MockDaSpec>;

generate_optimistic_runtime!(TestBlobStorageRuntime <= value_setter: ValueSetter<S>);

/// Returns the last `k` slot receipts
pub fn last_slot_receipts<
    K: KernelSlotHooks<S, Da> + BlobSelector<MockDaSpec, BlobType = BlobDataWithId>,
>(
    runner: &TestRunner<K>,
    k: usize,
) -> &[SlotReceipt<MockDaSpec>] {
    assert!(
        k <= runner.receipts().len(),
        "k must be less than or equal to the number of slots. k={}, number of slots={}",
        k,
        runner.receipts().len()
    );
    &runner.receipts()[runner.receipts().len() - k..]
}

/// Formats a batch receipt into a tuple of (batch_hash, sender) used for testing the blob storage.
fn format_batch_receipts(
    batch_receipts: &[BatchReceipt<MockDaSpec>],
) -> Vec<([u8; 32], <MockDaSpec as DaSpec>::Address)> {
    batch_receipts
        .iter()
        .map(|b| (b.batch_hash, b.inner.da_address))
        .collect::<Vec<_>>()
}

/// This helper method asserts that given slots to send and an expected order of receipts, the
/// [`TestRunner`] will emit the receipts in the expected order. This helper method is
/// used in [`helpers_basic_kernel::assert_blobs_are_correctly_received_basic_kernel`] and [`helpers_soft_confirmations::assert_blobs_are_correctly_received_soft_confirmation`].
fn assert_blobs_are_correctly_received_helper<
    K: KernelSlotHooks<S, MockDaSpec> + BlobSelector<MockDaSpec, BlobType = BlobDataWithId>,
>(
    slots_to_send: Vec<RelevantBlobs<MockBlob>>,
    receive_order: Vec<Vec<usize>>,
    runner: &mut TestRunner<K>,
) {
    for slot in slots_to_send.clone() {
        runner.execute::<RelevantBlobs<MockBlob>, ValueSetter<S>>(slot, None);
    }

    // If this inequality is verified, it means that we need to run a few empty slots because
    // we are waiting for the blobs to be deferred.
    if receive_order.len() > slots_to_send.len() {
        runner.advance_slots(receive_order.len() - slots_to_send.len());
    }

    assert!(runner.receipts().len() >= receive_order.len(), "The execution has not produced enough receipts! Expected at least {} receipts, but got {}.", 
        receive_order.len(), runner.receipts().len());

    // We get all the receipts we received during the execution.
    let received_slots = last_slot_receipts(runner, receive_order.len())
        .iter()
        .map(|s| format_batch_receipts(s.batch_receipts()))
        .collect::<Vec<_>>();

    // We get all the blobs we sent during the execution. We flatten the map to have the list of blobs independently of the slots.
    let sent_slots = slots_to_send
        .iter()
        .flat_map(|blobs| {
            blobs
                .batch_blobs
                .iter()
                .map(|blob| (blob.hash(), blob.sender()))
        })
        .collect::<Vec<_>>();

    // We check that the blobs we received are the ones we sent in the correct order.
    for (received_slot_num, sent_blob_nums) in receive_order.iter().enumerate() {
        assert_eq!(
            sent_blob_nums.len(),
            received_slots[received_slot_num].len(),

            "We have not received the expected number of blobs for the slot {}. Expected {}, but got {}.",
            received_slot_num,
            sent_blob_nums.len(),
            received_slots[received_slot_num].len()
        );

        for (received_batch_num, sent_blob_num) in sent_blob_nums.iter().enumerate() {
            assert_eq!(
                sent_slots[*sent_blob_num].0,
                MockHash(received_slots[received_slot_num][received_batch_num].0),
                "The blob hash for the blob number {} in the slot {} is not correct.",
                received_batch_num,
                received_slot_num,
            );

            assert_eq!(
                sent_slots[*sent_blob_num].1,
                received_slots[received_slot_num][received_batch_num].1,
                "The blob sender for the blob number {} in the slot {} is not correct.",
                received_batch_num,
                received_slot_num,
            );
        }
    }
}
