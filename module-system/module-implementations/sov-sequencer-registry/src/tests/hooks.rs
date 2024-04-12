use sov_mock_da::{MockAddress, MockDaSpec};
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::hooks::ApplyBatchHooks;

use crate::tests::helpers::{
    Da, TestSequencer, GENESIS_SEQUENCER_DA_ADDRESS, INITIAL_BALANCE, LOCKED_AMOUNT,
    UNKNOWN_SEQUENCER_DA_ADDRESS,
};
use crate::{SequencerOutcome, SequencerRegistry};

type S = sov_test_utils::TestSpec;

/// Tests that the `begin_batch_hook` passes if the sequencer is registered.
#[test]
fn begin_batch_hook_known_sequencer() {
    let (test_sequencer, mut working_set) = TestSequencer::initialize_test(INITIAL_BALANCE, false);

    let balance_after_genesis = test_sequencer
        .query_sequencer_balance(&mut working_set)
        .unwrap();

    assert_eq!(INITIAL_BALANCE - LOCKED_AMOUNT, balance_after_genesis);

    let genesis_sequencer_da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);

    let mut test_batch = BatchWithId {
        txs: Vec::new(),
        id: [0u8; 32],
    };

    let mut state_checkpoint = working_set.checkpoint().0;
    test_sequencer
        .registry
        .begin_batch_hook(
            &mut test_batch,
            &genesis_sequencer_da_address,
            &mut state_checkpoint,
        )
        .unwrap();

    let resp = test_sequencer
        .query_sequencer_balance(&mut state_checkpoint)
        .unwrap();
    assert_eq!(balance_after_genesis, resp);
    let resp = test_sequencer
        .registry
        .resolve_da_address(&genesis_sequencer_da_address, &mut state_checkpoint);
    assert!(resp.is_some());
}

/// Tests that the `begin_batch_hook` returns an error if the sequencer is not registered.
#[test]
fn begin_batch_hook_unknown_sequencer() {
    let (test_sequencer, working_set) = TestSequencer::initialize_test(INITIAL_BALANCE, false);

    let mut test_batch = BatchWithId {
        txs: Vec::new(),
        id: [0u8; 32],
    };

    let mut state_checkpoint = working_set.checkpoint().0;
    let result = test_sequencer.registry.begin_batch_hook(
        &mut test_batch,
        &MockAddress::from(UNKNOWN_SEQUENCER_DA_ADDRESS),
        &mut state_checkpoint,
    );
    assert!(result.is_err());
    let expected_message = format!(
        "sender {} is not allowed to submit blobs",
        MockAddress::from(UNKNOWN_SEQUENCER_DA_ADDRESS)
    );
    let actual_message = result.err().unwrap().to_string();
    assert_eq!(expected_message, actual_message);
}

/// Tests that calling `begin_batch_hook` following by `end_batch_hook` succeeds if the sequencer is registered.
#[test]
fn end_batch_hook_success() {
    let (test_sequencer, mut working_set) = TestSequencer::initialize_test(INITIAL_BALANCE, false);
    let balance_after_genesis = test_sequencer
        .query_sequencer_balance(&mut working_set)
        .unwrap();

    let genesis_sequencer_da_address = MockAddress::from(GENESIS_SEQUENCER_DA_ADDRESS);
    let mut test_batch = BatchWithId {
        txs: Vec::new(),
        id: [0u8; 32],
    };

    let mut state_checkpoint = working_set.checkpoint().0;
    test_sequencer
        .registry
        .begin_batch_hook(
            &mut test_batch,
            &genesis_sequencer_da_address,
            &mut state_checkpoint,
        )
        .unwrap();

    <SequencerRegistry<S, Da> as ApplyBatchHooks<MockDaSpec>>::end_batch_hook(
        &test_sequencer.registry,
        SequencerOutcome::Rewarded(0),
        &genesis_sequencer_da_address,
        &mut state_checkpoint,
    );
    let resp = test_sequencer
        .query_sequencer_balance(&mut state_checkpoint)
        .unwrap();
    assert_eq!(balance_after_genesis, resp);
    let resp = test_sequencer
        .registry
        .resolve_da_address(&genesis_sequencer_da_address, &mut state_checkpoint);
    assert!(resp.is_some());
}
