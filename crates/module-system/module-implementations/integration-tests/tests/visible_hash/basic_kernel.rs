use sov_modules_api::hooks::SlotHooks;
use sov_modules_api::{Spec, Storage};
use sov_state::{ProvableNamespace, StateRoot};
use sov_test_utils::{generate_bare_runtime, impl_standard_runtime_authenticator};

use crate::visible_hash::{
    last_state_root_closure, FinalizeHook, HighLevelOptimisticGenesisConfig, TestClosureArgs,
    TestUser, TestVisibleHashModule, S,
};

generate_bare_runtime! {
    name: TestVisibleHashRuntime,
    modules: [visible_hash_module: TestVisibleHashModule<S>],
    operating_mode: sov_modules_api::OperatingMode::Optimistic,
    minimal_genesis_config_type: sov_test_utils::runtime::genesis::optimistic::MinimalOptimisticGenesisConfig<S>,
    impl_hooks: [ApplyBatchHooks, KernelSlotHooks, TxHooks],
    runtime_trait_impl_bounds: [],
    kernel_type: sov_kernels::basic::BasicKernel<'a, S>
}

impl_standard_runtime_authenticator!(TestVisibleHashRuntime<S>);

type TestRunner<RT> = sov_test_utils::runtime::TestRunner<RT, S>;

type RT = TestVisibleHashRuntime<S>;

impl<S: Spec> SlotHooks for TestVisibleHashRuntime<S> {
    type Spec = S;

    fn begin_slot_hook(
        &self,
        visible_hash: &<<S as Spec>::Storage as Storage>::Root,
        state: &mut sov_modules_api::StateCheckpoint<<Self::Spec as Spec>::Storage>,
    ) {
        self.visible_hash_module
            .begin_slot_hook(visible_hash, state);
    }
}

impl<S: Spec> FinalizeHook for TestVisibleHashRuntime<S> {
    type Spec = S;

    fn finalize_hook(
        &self,
        root_hash: &<<S as Spec>::Storage as Storage>::Root,
        state: &mut impl sov_modules_api::AccessoryStateReaderAndWriter,
    ) {
        self.visible_hash_module.finalize_hook(root_hash, state);
    }
}

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
