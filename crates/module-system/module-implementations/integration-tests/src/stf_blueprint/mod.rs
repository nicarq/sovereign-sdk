mod registered;

use sov_bank::Bank;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{ApiStateAccessor, DaSpec, Spec};
use sov_modules_stf_blueprint::TxProcessingError;
use sov_sequencer_registry::SequencerRegistry;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::{config_gas_token_id, Payable, TestRunner};
use sov_test_utils::{
    generate_optimistic_runtime, TestSequencer, TestUser, TransactionTestCase,
    TEST_DEFAULT_USER_BALANCE,
};
use sov_value_setter::ValueSetter;

type S = sov_test_utils::TestSpec;

generate_optimistic_runtime!(IntegTestRuntime <= value_setter: ValueSetter<S>);

pub(crate) fn get_balance(payable: impl Payable<S>, state: &mut ApiStateAccessor<S>) -> u64 {
    Bank::<S>::default()
        .get_balance_of(payable, config_gas_token_id(), state)
        .unwrap_infallible()
        .unwrap()
}

pub(crate) fn get_seq_bond(
    sequencer_da_address: &<<S as Spec>::Da as DaSpec>::Address,
    state: &mut ApiStateAccessor<S>,
) -> u64 {
    let sequencer_module = SequencerRegistry::<S>::default();
    sequencer_module
        .is_sender_allowed(sequencer_da_address, state)
        .unwrap()
        .balance
}

pub(crate) fn setup() -> (
    TestRunner<IntegTestRuntime<S>, S>,
    TestUser<S>,
    TestSequencer<S>,
) {
    let mut genesis_config = HighLevelOptimisticGenesisConfig::generate();
    genesis_config
        .additional_accounts
        .push(TestUser::<S>::generate(TEST_DEFAULT_USER_BALANCE));

    let admin_account = genesis_config.additional_accounts[0].clone();

    let genesis = GenesisConfig::from_minimal_config(
        genesis_config.clone().into(),
        sov_value_setter::ValueSetterConfig {
            admin: admin_account.address(),
        },
    );

    let runner: TestRunner<IntegTestRuntime<S>, S> =
        TestRunner::new_with_genesis(genesis.into_genesis_params(), Default::default());

    let admin_account = genesis_config.additional_accounts[0].clone();
    let sequencer_account = genesis_config.initial_sequencer.clone();

    (runner, admin_account, sequencer_account)
}
