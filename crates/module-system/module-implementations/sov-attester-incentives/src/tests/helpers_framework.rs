use sov_bank::GAS_TOKEN_ID;
use sov_mock_da::MockDaSpec;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_test_utils::runtime::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::{AttesterIncentives, Bank, TestRunner};
use sov_test_utils::{
    generate_optimistic_runtime, SlotTestCase, TestAttester, TestChallenger, TestUser,
};

pub(crate) type S = sov_test_utils::TestSpec;

pub(crate) type TestAttesterIncentives = AttesterIncentives<S, MockDaSpec>;

pub(crate) type RT = AttesterRuntime<S, MockDaSpec>;

generate_optimistic_runtime!(AttesterRuntime <= );

pub type SetupParams = (
    TestRunner<RT, S>,
    TestAttester<S>,
    TestChallenger<S>,
    TestUser<S>,
);

/// Helper that sets up the tests and checks that the genesis state is valid.
pub(crate) fn setup() -> SetupParams {
    // Generate a genesis config, then overwrite the attester key/address with ones that
    // we know. We leave the other values untouched.
    let genesis_config = HighLevelOptimisticGenesisConfig::generate_with_additional_accounts(1);

    let genesis_attester = genesis_config.initial_attester.clone();

    let attester_address = genesis_attester.user_info.address();
    let attester_bond = genesis_attester.bond;
    let attester_balance = genesis_attester.user_info.available_balance;

    let genesis_challenger = genesis_config.initial_challenger.clone();

    let additional_account = genesis_config.additional_accounts.first().unwrap().clone();

    // Run genesis registering the attester and sequencer we've generated.
    let genesis = GenesisConfig::from_minimal_config(genesis_config.into());

    let mut runner =
        TestRunner::new_with_genesis(genesis.into_genesis_params(), AttesterRuntime::default());

    // Execute an empty slot to make sure the genesis is valid
    runner.execute_slots::<TestAttesterIncentives>(vec![
        // Start by checking the attester balance and bond.
        SlotTestCase::empty().with_end_slot_hook(Box::new(move |state| {
            // Check that the attester account is bonded
            assert_eq!(
                TestAttesterIncentives::default()
                    .bonded_attesters
                    .get(&attester_address, state)
                    .unwrap(),
                Some(attester_bond),
                "The genesis attester should be bonded"
            );

            // Check the balance of the attester is equal to the free balance
            assert_eq!(
                Bank::<S>::default()
                    .get_balance_of(&attester_address, GAS_TOKEN_ID, state)
                    .unwrap_infallible(),
                Some(attester_balance),
                "The balance of the attester should be equal to the free balance"
            );
        })),
    ]);

    (
        runner,
        genesis_attester,
        genesis_challenger,
        additional_account,
    )
}
