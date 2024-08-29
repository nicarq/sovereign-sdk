use std::collections::HashMap;

use sov_chain_state::ChainStateConfig;
use sov_kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_mock_da::{MockAddress, MockBlob, MockDaSpec};
use sov_modules_api::{CryptoSpec, Spec};
use sov_rollup_interface::da::RelevantBlobs;
use sov_sequencer_registry::SequencerRegistry;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::{TestRunnerWithKernel, ValueSetter, ValueSetterConfig};
use sov_test_utils::{
    generate_optimistic_runtime, AsUser, BatchType, TestSequencer, TestUser,
    TEST_DEFAULT_USER_STAKE,
};

mod blob_storage_tests;
mod capability_tests;

pub type S = sov_test_utils::TestSpec;

generate_optimistic_runtime!(TestBlobStorageRuntime <= value_setter: ValueSetter<S>);

pub type RT = TestBlobStorageRuntime<S, MockDaSpec>;

pub type TestRunner<K> = TestRunnerWithKernel<RT, K, S>;

pub type SlotConfigInfo = Vec<TestSequencer<S, MockDaSpec>>;

pub struct TestData<S: Spec> {
    pub user: TestUser<S>,
    pub preferred_sequencer: TestSequencer<S, MockDaSpec>,
    pub regular_sequencer: TestSequencer<S, MockDaSpec>,
}

pub fn setup() -> (TestData<S>, TestRunner<BasicKernel<S, MockDaSpec>>) {
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

    let runner = TestRunnerWithKernel::<_, BasicKernel<S, MockDaSpec>, _>::new_with_genesis(
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

/// Sets up a test runtime and returns a [`TestData`] struct.
pub fn setup_with_registration() -> (TestData<S>, TestRunner<BasicKernel<S, MockDaSpec>>) {
    let (test_data, mut runner) = setup();

    let regular_sequencer = &test_data.regular_sequencer;
    let regular_sequencer_da_address = regular_sequencer.da_address;

    runner.execute(
        regular_sequencer.create_plain_message::<SequencerRegistry<S, MockDaSpec>>(
            sov_sequencer_registry::CallMessage::Register {
                da_address: regular_sequencer_da_address.as_ref().to_vec(),
                amount: TEST_DEFAULT_USER_STAKE,
            },
        ),
        None,
    );

    (test_data, runner)
}

/// Builds a [`RelevantBlobs`] struct from a list of [`SlotConfigInfo`]s.
/// This struct populates the batches with simple [`ValueSetter`] messages. One
/// can specify special sequencer addresses for each batch.
fn build_blobs(
    admin: &TestUser<S>,
    slot_info: Vec<SlotConfigInfo>,
    nonces: &mut HashMap<<<S as Spec>::CryptoSpec as CryptoSpec>::PublicKey, u64>,
    runner: &mut TestRunner<BasicKernel<S, MockDaSpec>>,
) -> RelevantBlobs<MockBlob> {
    let batches = slot_info
        .into_iter()
        .flat_map(|batches_config_info| {
            let mut batches = Vec::new();
            for (i, sequencer) in batches_config_info.into_iter().enumerate() {
                batches.push((
                    BatchType(vec![admin.create_plain_message::<ValueSetter<S>>(
                        sov_value_setter::CallMessage::SetValue((i + 1) as u32),
                    )]),
                    sequencer.da_address,
                ));
            }

            batches
        })
        .collect::<Vec<_>>();

    runner.query_state(|state| {
        TestRunner::<BasicKernel<S, MockDaSpec>>::batches_to_blobs(batches, nonces, state)
    })
}
