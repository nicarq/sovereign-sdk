use sov_blob_storage::config_unregistered_blobs_per_slot;
use sov_mock_da::{MockBlob, MockDaSpec};
use sov_modules_api::capabilities::{BlobSelector, KernelSlotHooks, KernelWithSlotMapping};
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{BlobDataWithId, CryptoSpec, Spec};
use sov_rollup_interface::da::RelevantBlobs;
use sov_sequencer_registry::SequencerRegistry;
use sov_test_utils::{AsUser, SequencerInfo, TestSequencer};

use crate::helpers_basic_kernel::{build_basic_blobs, setup_basic_kernel};
use crate::helpers_soft_confirmations::{
    build_soft_confirmation_blobs, setup_soft_confirmation_kernel,
};
use crate::{HashMap, TestData, TestRunner, RT, S};

fn make_unregistered_blobs<
    K: KernelSlotHooks<S, MockDaSpec>
        + BlobSelector<MockDaSpec, BlobType = BlobDataWithId>
        + KernelWithSlotMapping<S>,
>(
    num_blobs: u64,
    sender: &TestSequencer<S, MockDaSpec>,
    nonces: &mut HashMap<<<S as Spec>::CryptoSpec as CryptoSpec>::PublicKey, u64>,
    runner: &mut TestRunner<K>,
) -> Vec<MockBlob> {
    let user_stake_value = runner.query_state(|state| {
        SequencerRegistry::<S, MockDaSpec>::default()
            .get_coins_to_lock(state)
            .unwrap_infallible()
            .amount
    });

    (0..num_blobs)
        .map(|_| {
            let tx = sender.create_plain_message::<SequencerRegistry<S, MockDaSpec>>(
                sov_sequencer_registry::CallMessage::Register {
                    da_address: sender.da_address.as_ref().to_vec(),
                    amount: user_stake_value,
                },
            );

            let raw_tx =
                runner.query_state(|state| tx.to_serialized_authenticated_tx::<RT>(nonces, state));

            MockBlob::new_with_hash(borsh::to_vec(&raw_tx).unwrap(), sender.da_address)
        })
        .collect::<Vec<_>>()
}

/// Tries to send too many blobs from a non-registered sequencer and hit rate limits.
#[test]
fn blobs_from_non_registered_sequencers_are_limited_to_set_amount() {
    let (
        TestData {
            regular_sequencer: non_registered_sequencer,
            ..
        },
        mut runner,
    ) = setup_basic_kernel();

    let mut nonces = HashMap::new();

    // Make more unregistered blobs than the limit
    let unregistered_blobs = make_unregistered_blobs(
        config_unregistered_blobs_per_slot() + 1,
        &non_registered_sequencer,
        &mut nonces,
        &mut runner,
    );

    let unregistered_blobs = RelevantBlobs {
        proof_blobs: Default::default(),
        batch_blobs: unregistered_blobs,
    };

    // Send them
    let result = runner
        .execute::<RelevantBlobs<MockBlob>, SequencerRegistry<S, MockDaSpec>>(unregistered_blobs);

    // Assert that the number of blobs received is below the [`UNREGISTERED_BLOBS_PER_SLOT`] limit
    assert_eq!(
        result.batch_receipts.len(),
        config_unregistered_blobs_per_slot() as usize,
        "The number of blobs received should be equal to `UNREGISTERED_BLOBS_PER_SLOT`"
    );
}

#[test]
fn blobs_from_non_registered_sequencers_are_limited_to_set_amount_soft_confirmation() {
    let (
        TestData {
            preferred_sequencer,
            regular_sequencer: non_registered_sequencer,
            ..
        },
        mut runner,
    ) = setup_soft_confirmation_kernel();

    let mut nonces = HashMap::new();

    // Make more unregistered blobs than the limit
    let mut unregistered_blobs = make_unregistered_blobs(
        config_unregistered_blobs_per_slot() + 1,
        &non_registered_sequencer,
        &mut nonces,
        &mut runner,
    );

    let mut slot_to_send = build_soft_confirmation_blobs(
        &vec![(
            preferred_sequencer.clone(),
            SequencerInfo::Preferred {
                slots_to_advance: 1,
                sequence_number: 0,
            },
        )],
        &mut nonces,
        &mut runner,
    );

    slot_to_send.batch_blobs.append(&mut unregistered_blobs);

    // Send them
    let result =
        runner.execute::<RelevantBlobs<MockBlob>, SequencerRegistry<S, MockDaSpec>>(slot_to_send);

    // Assert that the number of blobs received is below the [`UNREGISTERED_BLOBS_PER_SLOT`] limit
    assert_eq!(
        result.batch_receipts.len(),
        1 + config_unregistered_blobs_per_slot() as usize,
        "The number of blobs received should be equal to `UNREGISTERED_BLOBS_PER_SLOT` plus 1 (the preferred blob)"
    );
}

#[test]
fn blobs_from_non_registered_sequencers_base_sequencing() {
    let (
        TestData {
            preferred_sequencer,
            regular_sequencer: non_registered_sequencer,
            ..
        },
        mut runner,
    ) = setup_basic_kernel();

    let mut nonces = HashMap::new();

    // Make more unregistered blobs than the limit
    let mut unregistered_blobs = make_unregistered_blobs(
        config_unregistered_blobs_per_slot() + 1,
        &non_registered_sequencer,
        &mut nonces,
        &mut runner,
    );

    let mut slot_to_send = build_basic_blobs(
        &vec![preferred_sequencer.clone(); 4],
        &mut nonces,
        &mut runner,
    );

    slot_to_send.batch_blobs.append(&mut unregistered_blobs);

    // Send them
    let result =
        runner.execute::<RelevantBlobs<MockBlob>, SequencerRegistry<S, MockDaSpec>>(slot_to_send);

    // Assert that the number of blobs received is below the [`UNREGISTERED_BLOBS_PER_SLOT`] limit
    assert_eq!(
        result.batch_receipts.len(),
        4 + config_unregistered_blobs_per_slot() as usize,
        "The number of blobs received should be equal to `UNREGISTERED_BLOBS_PER_SLOT` plus 4 (the registered blobs)"
    );
}
