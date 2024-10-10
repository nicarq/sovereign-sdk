use sov_kernels::basic::BasicKernel;
use sov_mock_da::{MockAddress, MockBlob, MockDaSpec};
use sov_modules_api::{CryptoSpec, Gas, GasSpec, Spec};
use sov_rollup_interface::da::RelevantBlobs;
use sov_test_utils::runtime::genesis::zk::config::HighLevelZkGenesisConfig;
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
    let genesis_config = HighLevelZkGenesisConfig::generate_with_additional_accounts(2);
    let preferred_sequencer = genesis_config.initial_sequencer.clone();
    let user_account = genesis_config.additional_accounts.first().unwrap().clone();

    let regular_sequencer = genesis_config.additional_accounts[1].clone();
    let regular_sequencer_da_address = MockAddress::new([42; 32]);

    let user_stake = <S as Spec>::Gas::from(TEST_DEFAULT_USER_STAKE);
    let user_stake_value = user_stake.value(&S::initial_base_fee_per_gas());

    let regular_sequencer = TestSequencer {
        user_info: regular_sequencer,
        da_address: regular_sequencer_da_address,
        bond: user_stake_value,
    };

    // Run genesis registering the attester and sequencer we've generated.
    let genesis = GenesisConfig::from_minimal_config(
        genesis_config.into(),
        ValueSetterConfig {
            admin: user_account.address(),
        },
    );

    let runner = TestRunner::<BasicKernel<S, MockDaSpec>>::new_with_genesis(
        genesis.into_genesis_params(),
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
/// - Slot 1: Send [ (Blob 0), (Blob 1), (Blob 2) ] | Receive [ Blob 0, Blob 1, Blob 2 ]
/// - Slot 2: Send [ (Blob 3), (Blob 4) ] | Receive [ Blob 3, Blob 4 ]
/// - Slot 3: Send [] | Receive [ ]
/// - Slot 4: Send [] | Receive [ ]
///
/// Then the `receive_order` parameter should be [ [0, 1, 2], [3, 4], [0], [0] ].
///
/// The `virtual_slot_heights_increases` parameter indicates the virtual slot heights that we expect to advance.
/// In the situation above: we would have [1, 1, 0, 0] for the `virtual_slot_heights_increases` parameter.
pub fn assert_blobs_are_correctly_received_basic_kernel(
    sending_order: Vec<Vec<TestSequencer<S, MockDaSpec>>>,
    receive_order: Vec<Vec<usize>>,
    virtual_slot_heights_increases: Vec<u64>,
    runner: &mut TestRunner<BasicKernel<S, MockDaSpec>>,
) {
    let mut nonces = HashMap::new();

    let slots_to_send = sending_order
        .iter()
        .map(|blobs_slot_info| build_basic_blobs(blobs_slot_info, &mut nonces, runner))
        .collect::<Vec<_>>();

    assert_blobs_are_correctly_received_helper(
        slots_to_send,
        receive_order,
        virtual_slot_heights_increases,
        runner,
    );
}
