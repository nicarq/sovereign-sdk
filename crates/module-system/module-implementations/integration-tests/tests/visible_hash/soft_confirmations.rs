use sov_modules_api::hooks::SlotHooks;
use sov_modules_api::macros::config_value;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{Spec, Storage};
use sov_state::{ProvableNamespace, StateRoot};
use sov_test_utils::{generate_bare_runtime, impl_standard_runtime_authenticator, TestSequencer};

use crate::visible_hash::{
    last_state_root_closure, FinalizeHook, HighLevelOptimisticGenesisConfig, TestClosureArgs,
    TestRunner, TestUser, TestVisibleHashModule, S,
};

generate_bare_runtime! {
    name: TestVisibleHashRuntime,
    modules: [visible_hash_module: TestVisibleHashModule<S>],
    operating_mode: sov_modules_api::OperatingMode::Optimistic,
    minimal_genesis_config_type: sov_test_utils::runtime::genesis::optimistic::MinimalOptimisticGenesisConfig<S>,
    impl_hooks: [ApplyBatchHooks, KernelSlotHooks, TxHooks],
    runtime_trait_impl_bounds: [],
    kernel_type: sov_kernels::soft_confirmations::SoftConfirmationsKernel<'a, S>
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

fn setup() -> (TestUser<S>, TestSequencer<S>, TestRunner<RT>) {
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);

    let user = genesis_config.additional_accounts.first().unwrap().clone();
    let sequencer = genesis_config.initial_sequencer.clone();

    let genesis = GenesisConfig::from_minimal_config(genesis_config.into(), ());

    let runner = TestRunner::new_with_genesis(genesis.into_genesis_params(), RT::default());

    (user, sequencer, runner)
}

/// Tests that the visible kernel hash does not update for each slot for the soft confirmations Kernel
/// The finalize hook hash should always match the most recent slot hash.
#[test]
fn visible_hash_soft_confirmations_kernel() {
    let (_, _, mut runner) = setup();

    let genesis_hash = *runner.state_root();

    let num_slots: u64 = config_value!("DEFERRED_SLOTS_COUNT") - 1;

    // We run `DEFERRED_SLOTS_COUNT` - 1 slots. The user hash should not update
    last_state_root_closure(
        &mut |TestClosureArgs {
                  finalize_hook_hash,
                  current_slot_hash,
                  prev_slot_hash,
                  prev_finalize_hook_hash,
                  ..
              }| {
            assert_eq!(
                current_slot_hash.namespace_root(ProvableNamespace::User),
                genesis_hash.namespace_root(ProvableNamespace::User),
                "The user state root should not update until the virtual state root updates"
            );

            assert_ne!(
                current_slot_hash.namespace_root(ProvableNamespace::Kernel),
                prev_slot_hash.namespace_root(ProvableNamespace::Kernel),
                "The kernel state root should update at every slot"
            );

            assert_eq!(
                current_slot_hash.namespace_root(ProvableNamespace::User),
                prev_slot_hash.namespace_root(ProvableNamespace::User),
                "The user state root should not update"
            );

            assert_eq!(
                finalize_hook_hash, current_slot_hash,
                "The finalize hash and the current hash should always be the same"
            );

            assert_eq!(
                prev_finalize_hook_hash, prev_slot_hash,
                "The previous finalize hash should match the previous slot hash"
            );
        },
        &mut runner,
        num_slots,
    );

    // We run 1 more slot. The user hash should update
    last_state_root_closure(
        &mut |TestClosureArgs {
                  current_slot_hash,
                  prev_slot_hash,
                  finalize_hook_hash,
                  ..
              }| {
            assert_ne!(
                current_slot_hash.namespace_root(ProvableNamespace::User),
                prev_slot_hash.namespace_root(ProvableNamespace::User),
                "The user state root should update because the kernel slot hook updates"
            );

            assert_ne!(
                current_slot_hash.namespace_root(ProvableNamespace::Kernel),
                prev_slot_hash.namespace_root(ProvableNamespace::Kernel),
                "The kernel state root should update at every slot"
            );

            // The finalize hook hash should always match the most recent slot hash
            assert_eq!(
                finalize_hook_hash, current_slot_hash,
                "The finalize hash should always match the most recent slot hash"
            );
        },
        &mut runner,
        1,
    );
}

#[test]
fn begin_slot_hash_soft_confirmations_kernel() {
    let (_, _, mut runner) = setup();

    let genesis_hash = *runner.state_root();

    let num_slots: u64 = config_value!("DEFERRED_SLOTS_COUNT") - 1;

    let module = TestVisibleHashModule::<S>::default();

    // We run `DEFERRED_SLOTS_COUNT` - 1 slots. The user hash should not update
    runner.advance_slots(num_slots as usize);

    // We run 1 more slot. The begin slot hash should update
    last_state_root_closure(
        &mut |TestClosureArgs {
                  begin_slot_hash, ..
              }| {
            assert_eq!(begin_slot_hash.unwrap(), genesis_hash);
        },
        &mut runner,
        1,
    );

    let expected_begin_slot_hash = runner.query_state(|state| {
        let pre_state_root = runner.state_root();

        let user_root = pre_state_root.namespace_root(sov_state::ProvableNamespace::User);

        let root_at_height = module
            .chain_state
            .root_at_height(runner.visible_rollup_height(), state)
            .unwrap_infallible()
            .unwrap();

        let kernel_root = root_at_height.namespace_root(sov_state::ProvableNamespace::Kernel);

        <<S as Spec>::Storage as Storage>::Root::from_namespace_roots(user_root, kernel_root)
    });

    let slot_hash_at_height_one = runner.query_state(|state| {
        module
            .chain_state
            .root_at_height(1, state)
            .unwrap_infallible()
            .unwrap()
    });

    // We run 1 more slot. The begin slot hash should update
    last_state_root_closure(
        &mut |TestClosureArgs {
                  begin_slot_hash, ..
              }| {
            assert_eq!(
                begin_slot_hash.unwrap(),
                expected_begin_slot_hash,
                "The begin slot hash should be the same as the computed visible hash"
            );

            assert_ne!(
                begin_slot_hash.unwrap(), slot_hash_at_height_one,
                "The begin slot hash should be different than the slot hash at height 1. That is because the user space root should have updated afterwards"
            );
        },
        &mut runner,
        1,
    );
}
