use sov_mock_da::MockDaSpec;
use sov_modules_api::capabilities::AllowedSequencer;
use sov_modules_api::ApiStateAccessor;
use sov_sequencer_registry::{SequencerRegistry, SequencerRegistryError};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::{TestRunner, ValueSetter, ValueSetterConfig};
use sov_test_utils::{
    generate_optimistic_runtime, TestSequencer, TestUser, TEST_DEFAULT_USER_STAKE,
};

pub type S = sov_test_utils::TestSpec;
pub type Da = MockDaSpec;

pub const NON_DEFAULT_SEQUENCER_DA_ADDRESS: [u8; 32] = [1; 32];
pub const ANOTHER_SEQUENCER_DA_ADDRESS: [u8; 32] = [2; 32];

generate_optimistic_runtime!(TestRuntime <= value_setter: ValueSetter<S>);

pub(crate) type RT = TestRuntime<S, Da>;

pub(crate) type TestSequencerRegistry = SequencerRegistry<S, Da>;

pub(crate) type TestSequencerRegistryError =
    SequencerRegistryError<S, MockDaSpec, ApiStateAccessor<S>>;

/// Defines the roles that are used in the sequencer registry tests.
pub struct TestRoles {
    /// The default sequencer.
    pub default_sequencer: TestSequencer<S, Da>,
    /// Another user that can be used to register a sequencer.
    pub additional_sequencer: TestUser<S>,
    /// The admin of the [`ValueSetter`] module.
    pub admin: TestUser<S>,
}

/// Simple helper that creates a test sequencer, initializes it with genesis data and verifies that the initialization was successful.
/// Returns a `TestSequencer` and two `TestUsers` that are used to test the sequencer registry, the first one is also the admin of the [`ValueSetter`] module.
pub fn setup() -> (TestRoles, TestRunner<TestRuntime<S, Da>, S>) {
    let genesis_config = HighLevelOptimisticGenesisConfig::generate_with_additional_accounts(2);

    let genesis_sequencer = genesis_config.initial_sequencer.clone();
    let genesis_sequencer_da_address = genesis_sequencer.da_address;
    let genesis_sequencer_balance = genesis_sequencer.user_info.available_gas_balance;
    let genesis_sequencer_address = genesis_sequencer.user_info.address();

    let admin = genesis_config.additional_accounts[0].clone();

    let other_sequencer = genesis_config.additional_accounts[1].clone();

    let value_setter_config = ValueSetterConfig {
        admin: admin.address(),
    };

    let genesis = GenesisConfig::from_minimal_config(genesis_config.into(), value_setter_config);

    let mut runner = TestRunner::new_with_genesis(genesis.into_genesis_params(), RT::default());

    runner.query_state(|state| {
        // Check that the sequencer account is bonded
        assert_eq!(
            TestSequencerRegistry::default()
                .is_sender_allowed(&genesis_sequencer_da_address, state),
            Ok(AllowedSequencer {
                address: genesis_sequencer_address,
                balance: TEST_DEFAULT_USER_STAKE,
            }),
            "The genesis attester should be bonded"
        );

        // Check the balance of the sequencer is equal to the free balance
        assert_eq!(
            TestRunner::<RT, S>::bank_gas_balance(&genesis_sequencer_address, state),
            Some(genesis_sequencer_balance),
            "The balance of the sequencer should be equal to the free balance"
        );
    });

    (
        TestRoles {
            default_sequencer: genesis_sequencer,
            additional_sequencer: other_sequencer,
            admin,
        },
        runner,
    )
}
