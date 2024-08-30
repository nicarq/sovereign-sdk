use sov_chain_state::ChainStateConfig;
use sov_kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_mock_da::{MockAddress, MockBlob, MockDaSpec};
use sov_modules_api::{CryptoSpec, Spec};
use sov_rollup_interface::da::RelevantBlobs;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::{BatchType, TestSequencer, TEST_DEFAULT_USER_STAKE};
use sov_value_setter::{ValueSetter, ValueSetterConfig};

use crate::{
    assert_blobs_are_correctly_received_helper, GenesisConfig, HashMap, SlotConfigInfo,
    TestBlobStorageRuntime, TestData, TestRunner, S,
};

/// Sets up a test runtime and returns a [`TestData`] struct.
pub fn setup_basic_kernel() -> (TestData<S>, TestRunner<BasicKernel<S, MockDaSpec>>) {
    // Generate a genesis config, then overwrite the attester key/address with ones that
    // we know. We leave the other values untouched.
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(2);

    let preferred_sequencer = genesis_config.initial_sequencer.clone();
    let user_account = genesis_config.additional_accounts.first().unwrap().clone();

    let regular_sequencer = genesis_config.additional_accounts[1].clone();
    let regular_sequencer_da_address = MockAddress::new([42; 32]);

    let regular_sequencer = TestSequencer {
        user_info: regular_sequencer,
        da_address: regular_sequencer_da_address,
        bond: TEST_DEFAULT_USER_STAKE,
    };

    // Run genesis registering the attester and sequencer we've generated.
    let genesis = GenesisConfig::from_minimal_config(
        genesis_config.into(),
        ValueSetterConfig {
            admin: user_account.address(),
        },
    );

    let runner = TestRunner::<BasicKernel<S, MockDaSpec>>::new_with_genesis(
        genesis.into_genesis_params_with_kernel(BasicKernelGenesisConfig {
            chain_state: ChainStateConfig {
                current_time: Default::default(),
                genesis_da_height: 0,
                inner_code_commitment: Default::default(),
                outer_code_commitment: Default::default(),
            },
        }),
        TestBlobStorageRuntime::default(),
    );

    (
        TestData {
            user: user_account,
            preferred_sequencer,
            regular_sequencer,
        },
        runner,
    )
}

/// Builds a [`RelevantBlobs`] struct from a list of [`BlobConfigInfo`]s.
/// This struct populates the batches with simple [`ValueSetter`] messages. One
/// can specify special sequencer addresses for each batch.
pub fn build_basic_blobs(
    slots_info: &SlotConfigInfo<TestSequencer<S, MockDaSpec>>,
    nonces: &mut HashMap<<<S as Spec>::CryptoSpec as CryptoSpec>::PublicKey, u64>,
    runner: &mut TestRunner<BasicKernel<S, MockDaSpec>>,
) -> RelevantBlobs<MockBlob> {
    let mut batches = Vec::new();
    for sequencer in slots_info {
        batches.push((BatchType(vec![]), sequencer.da_address));
    }

    runner.query_state(|state| {
        TestRunner::<BasicKernel<S, MockDaSpec>>::batches_to_blobs::<ValueSetter<S>>(
            batches, nonces, state,
        )
    })
}

/// This helper method asserts that given slots to send and an expected order of receipts, the
/// [`TestRunner`] will emit the receipts in the expected order.
///
/// The `receive_order` parameter is the list of indexes of the batches that we expect to receive.
///
/// Example: If we have the following situation:
/// - Slot 1: Send [ (Blob 0, sequencer A, Preferred { slots_to_advance: 1, sequence_number: 0 }), (Blob 1, sequencer B, Regular), (Blob 2, sequencer B, Regular) ] | Receive [ Blob 0 ]
/// - Slot 2: Send [ (Blob 3, sequencer A, Preferred { slots_to_advance: 1, sequence_number: 1 }), (Blob 4, sequencer B, Regular) ] | Receive [ Blob 3 ]
/// - Slot 3: Send [] | Receive [ Blob 1, Blob 2 ]
/// - Slot 4: Send [] | Receive [ Blob 4 ]
///
/// Then the `receive_order` parameter should be [ [0], [3], [1, 2], [4] ].
pub fn assert_blobs_are_correctly_received_basic_kernel(
    sending_order: Vec<Vec<TestSequencer<S, MockDaSpec>>>,
    receive_order: Vec<Vec<usize>>,
    runner: &mut TestRunner<BasicKernel<S, MockDaSpec>>,
) {
    let mut nonces = HashMap::new();

    let slots_to_send = sending_order
        .iter()
        .map(|blobs_slot_info| build_basic_blobs(blobs_slot_info, &mut nonces, runner))
        .collect::<Vec<_>>();

    assert_blobs_are_correctly_received_helper(slots_to_send, receive_order, runner);
}
