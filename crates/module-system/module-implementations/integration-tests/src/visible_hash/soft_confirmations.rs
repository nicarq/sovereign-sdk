use sov_chain_state::ChainState;
use sov_modules_api::hooks::SlotHooks;
use sov_modules_api::macros::config_value;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{Spec, Storage};
use sov_state::StateRoot;
use sov_test_utils::{generate_bare_runtime, impl_standard_runtime_authenticator};

use crate::visible_hash::{
    last_state_root_closure, FinalizeHook, HighLevelOptimisticGenesisConfig, ProvableNamespace,
    TestClosureArgs, TestRunner, TestUser, TestVisibleHashModule, S,
};

generate_bare_runtime! {
    name: TestVisibleHashRuntime,
    modules: [visible_hash_module: TestVisibleHashModule<S>],
    operating_mode: sov_modules_api::OperatingMode::Optimistic,
    minimal_genesis_config_type: sov_test_utils::runtime::genesis::optimistic::MinimalOptimisticGenesisConfig<S>,
    impl_hooks: [ApplyBatchHooks, TxHooks],
    runtime_trait_impl_bounds: [],
    kernel_type: sov_kernels::soft_confirmations::SoftConfirmationsKernel<S>
}

impl_standard_runtime_authenticator!(TestVisibleHashRuntime<S>);

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

/// Tests that the visible kernel hash does not update for each slot for the soft confirmations Kernel
/// The finalize hook hash should always match the most recent slot hash.
#[test]
fn visible_hash_soft_confirmations_kernel() {
    let (_, mut runner) = setup();

    let num_slots: u64 = config_value!("DEFERRED_SLOTS_COUNT") - 1;

    let genesis_hash = runner
        .state_root()
        .clone()
        .namespace_root(ProvableNamespace::Kernel);

    // We run `DEFERRED_SLOTS_COUNT` - 1 slots. The visible hash should not update
    last_state_root_closure(
        &mut |TestClosureArgs {
                  begin_slot_hash,
                  finalize_hook_hash,
                  current_slot_hash,
                  ..
              }| {
            assert_eq!(
                begin_slot_hash, genesis_hash,
                "The begin slot hash should match the genesis hash"
            );

            assert_ne!(
                finalize_hook_hash, begin_slot_hash,
                "The begin and finalize slot hashes should match"
            );

            // The finalize hook hash should always match the most recent slot hash
            assert_eq!(
                finalize_hook_hash, current_slot_hash,
                "The last finalize slot hash should match the current slot hash"
            );
        },
        &mut runner,
        num_slots,
    );

    // We expect that the new kernel root matches the one after the first transition (deferred update).
    let expected_visible_hash = runner.query_state(|state| {
        ChainState::<S>::default()
            .get_historical_transitions(1, state)
            .unwrap_infallible()
            .unwrap()
            .post_state_root()
            .namespace_root(ProvableNamespace::Kernel)
    });

    // We run 1 more slot. The visible kernel hash should update
    last_state_root_closure(
        &mut |TestClosureArgs {
                  begin_slot_hash,
                  finalize_hook_hash,
                  current_slot_hash,
                  ..
              }| {
            assert_ne!(
                begin_slot_hash, finalize_hook_hash,
                "The last begin and finalize kernel hashes should not match"
            );

            assert_ne!(
                begin_slot_hash, expected_visible_hash,
                "The begin kernel hash should match the hash of the first transition"
            );

            assert_ne!(
                finalize_hook_hash, expected_visible_hash,
                "The last finalize kernel hash should match the hash of the first transition"
            );

            // The finalize hook hash should always match the most recent slot hash
            assert_eq!(
                finalize_hook_hash, current_slot_hash,
                "The last finalize slot hash should match the current slot hash"
            );
        },
        &mut runner,
        1,
    );

    // We run 1 more slot. The visible begin slot kernel hash should update
    last_state_root_closure(
        &mut |TestClosureArgs {
                  begin_slot_hash, ..
              }| {
            assert_eq!(
                begin_slot_hash, expected_visible_hash,
                "The last begin slot kernel hash should match the hash of the first transition"
            );
        },
        &mut runner,
        1,
    );
}
