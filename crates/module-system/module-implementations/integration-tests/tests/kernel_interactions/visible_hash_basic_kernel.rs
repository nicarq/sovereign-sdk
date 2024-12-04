use sov_state::{ProvableNamespace, StateRoot};
use sov_test_utils::{generate_bare_runtime, impl_standard_runtime_authenticator};

use crate::kernel_interactions::{
    last_state_root_closure, HighLevelOptimisticGenesisConfig, TestClosureArgs, TestUser,
    TestVisibleHashModule, S,
};

generate_bare_runtime! {
    name: TestVisibleHashRuntime,
    modules: [visible_hash_module: TestVisibleHashModule<S>],
    operating_mode: sov_modules_api::OperatingMode::Optimistic,
    minimal_genesis_config_type: sov_test_utils::runtime::genesis::optimistic::MinimalOptimisticGenesisConfig<S>,
    gas_enforcer: bank: sov_test_utils::runtime::Bank<S>,
    runtime_trait_impl_bounds: [],
    kernel_type: sov_kernels::basic::BasicKernel<'a, S>
}

impl_standard_runtime_authenticator!(TestVisibleHashRuntime<S>);

type TestRunner<RT> = sov_test_utils::runtime::TestRunner<RT, S>;

type RT = TestVisibleHashRuntime<S>;

fn setup() -> (TestUser<S>, TestRunner<RT>) {
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);

    let user = genesis_config.additional_accounts.first().unwrap().clone();

    let genesis = GenesisConfig::from_minimal_config(genesis_config.into(), ());

    let runner = TestRunner::new_with_genesis(genesis.into_genesis_params(), RT::default());

    (user, runner)
}

/// Tests that the visible kernel hash updates for each slot for the basic Kernel
#[test]
fn visible_hash_basic_kernel() {
    let (_, mut runner) = setup();

    const NUM_SLOTS: u64 = 10;

    last_state_root_closure(
        &mut |TestClosureArgs {
                  prev_finalize_hook_hash,
                  prev_slot_hash,
                  finalize_hook_hash,
                  current_slot_hash,
                  begin_slot_hash,
              }| {
            assert_eq!(
                prev_finalize_hook_hash, prev_slot_hash,
                "The previous finalize hash should always match the previous slot hash"
            );

            assert_eq!(
                finalize_hook_hash, current_slot_hash,
                "The current finalize hash should always match the current slot hash"
            );

            assert_eq!(
                begin_slot_hash.unwrap(),
                prev_slot_hash,
                "The begin slot hash should be the same as the previous slot hash"
            );

            assert_ne!(
                current_slot_hash, prev_slot_hash,
                "The slot hash should always update"
            );

            assert_ne!(
                current_slot_hash.namespace_root(ProvableNamespace::Kernel),
                prev_slot_hash.namespace_root(ProvableNamespace::Kernel),
                "The kernel hash should always update in the basic kernel"
            );

            assert_ne!(
                current_slot_hash.namespace_root(ProvableNamespace::User),
                prev_slot_hash.namespace_root(ProvableNamespace::User),
                "The user hash should always update in the basic kernel"
            );
        },
        &mut runner,
        NUM_SLOTS,
    );
}
