use sov_chain_state::{ChainState, ChainStateConfig};
use sov_kernels::basic::{BasicKernel, BasicKernelGenesisConfig};
use sov_kernels::soft_confirmations::{
    SoftConfirmationsKernel, SoftConfirmationsKernelGenesisConfig,
};
use sov_mock_da::MockDaSpec;
use sov_modules_api::capabilities::{Kernel, KernelSlotHooks};
use sov_modules_api::hooks::{FinalizeHook, SlotHooks};
use sov_modules_api::macros::config_value;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{
    AccessoryStateReaderAndWriter, AccessoryStateVec, BlobDataWithId, CallResponse, Context,
    DaSpec, GenesisState, InfallibleStateAccessor, Module, ModuleError, ModuleId, ModuleInfo, Spec,
    StateVec, TxState,
};
use sov_state::{ProvableNamespace, StateRoot, Storage};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::{generate_bare_runtime, impl_standard_runtime_authenticator, TestUser};

type S = sov_test_utils::TestSpec;
type TestRunnerWithKernel<K> =
    sov_test_utils::runtime::TestRunnerWithKernel<TestVisibleHashRuntime<S, MockDaSpec>, K, S>;

#[derive(ModuleInfo, Clone)]
pub struct TestVisibleHashModule<S: Spec> {
    #[id]
    id: ModuleId,

    #[state]
    finalize_hook_hash: AccessoryStateVec<<S::Storage as Storage>::Root>,

    #[state]
    begin_slot_hash: StateVec<<S::Storage as Storage>::Root>,

    #[phantom]
    phantom: std::marker::PhantomData<S>,
}

impl<S: Spec> Module for TestVisibleHashModule<S> {
    type Spec = S;
    type Config = ();
    type CallMessage = ();
    type Event = ();

    fn genesis(
        &self,
        _config: &Self::Config,
        _state: &mut impl GenesisState<S>,
    ) -> Result<(), ModuleError> {
        Ok(())
    }

    fn call(
        &self,
        _msg: Self::CallMessage,
        _context: &Context<Self::Spec>,
        _state: &mut impl TxState<S>,
    ) -> Result<CallResponse, ModuleError> {
        Ok(Default::default())
    }
}

impl<S: Spec> TestVisibleHashModule<S> {
    fn begin_slot_hook(
        &self,
        visible_hash: &<S::Storage as Storage>::Root,
        state: &mut impl InfallibleStateAccessor,
    ) {
        self.begin_slot_hash
            .push(visible_hash, state)
            .unwrap_infallible();
    }

    fn finalize_hook(
        &self,
        visible_hash: &<S::Storage as Storage>::Root,
        state: &mut impl AccessoryStateReaderAndWriter,
    ) {
        self.finalize_hook_hash
            .push(visible_hash, state)
            .unwrap_infallible();
    }
}

generate_bare_runtime! {
    name: TestVisibleHashRuntime,
    modules: [visible_hash_module: TestVisibleHashModule<S>],
    operating_mode: sov_test_utils::runtime::OperatingMode::Optimistic,
    minimal_genesis_config_type: sov_test_utils::runtime::genesis::optimistic::MinimalOptimisticGenesisConfig<S, Da>,
    impl_hooks: [ApplyBatchHooks, TxHooks],
    runtime_trait_impl_bounds: []
}

impl_standard_runtime_authenticator!(TestVisibleHashRuntime<S, Da>);

impl<S: Spec, Da: DaSpec> SlotHooks for TestVisibleHashRuntime<S, Da> {
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

impl<S: Spec, Da: DaSpec> FinalizeHook for TestVisibleHashRuntime<S, Da> {
    type Spec = S;

    fn finalize_hook(
        &self,
        root_hash: &<<S as Spec>::Storage as Storage>::Root,
        state: &mut impl sov_modules_api::AccessoryStateReaderAndWriter,
    ) {
        self.visible_hash_module.finalize_hook(root_hash, state);
    }
}

fn setup<K>(kernel_config: K::GenesisConfig) -> (TestUser<S>, TestRunnerWithKernel<K>)
where
    K: KernelSlotHooks<S, MockDaSpec, BlobType = BlobDataWithId> + Kernel<<S as Spec>::Storage>,
{
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);

    let user = genesis_config.additional_accounts.first().unwrap().clone();

    let genesis = GenesisConfig::from_minimal_config(genesis_config.into(), ());

    let runner = TestRunnerWithKernel::<K>::new_with_genesis(
        genesis.into_genesis_params_with_kernel(kernel_config),
        TestVisibleHashRuntime::default(),
    );

    (user, runner)
}

struct TestClosureArgs {
    prev_finalize_hook_hash: [u8; 32],
    begin_slot_hash: [u8; 32],
    finalize_hook_hash: [u8; 32],
    current_slot_hash: [u8; 32],
}

/// A helper method for the visible hash tests. It advances the module state by `num_slots` and runs a closure with
/// the specified test arguments after each iteration.
fn last_state_root_closure<K>(
    test_closure: &mut impl FnMut(TestClosureArgs),
    runner: &mut TestRunnerWithKernel<K>,
    num_slots: u64,
) where
    K: KernelSlotHooks<S, MockDaSpec, BlobType = BlobDataWithId> + Kernel<<S as Spec>::Storage>,
{
    let module = TestVisibleHashModule::<S>::default();

    let mut prev_finalize_hook_hash = runner.query_state(|state| {
        module
            .finalize_hook_hash
            .get(0, state)
            .unwrap_infallible()
            .unwrap()
            .namespace_root(ProvableNamespace::Kernel)
    });

    for _ in 0..num_slots {
        runner.advance_slots(1_usize);

        runner.query_state(|state| {
            let begin_slot_hash = module
                .begin_slot_hash
                .last(state)
                .unwrap_infallible()
                .unwrap()
                .namespace_root(ProvableNamespace::Kernel);

            let finalize_hook_hash = module
                .finalize_hook_hash
                .last(state)
                .unwrap_infallible()
                .unwrap()
                .namespace_root(ProvableNamespace::Kernel);

            let current_slot_hash = runner
                .state_root()
                .clone()
                .namespace_root(ProvableNamespace::Kernel);

            test_closure(TestClosureArgs {
                begin_slot_hash,
                finalize_hook_hash,
                prev_finalize_hook_hash,
                current_slot_hash,
            });

            prev_finalize_hook_hash = finalize_hook_hash;
        });
    }
}

/// Tests that the visible kernel hash updates for each slot for the basic Kernel
#[test]
fn visible_hash_basic_kernel() {
    let (_, mut runner) = setup::<BasicKernel<S, MockDaSpec>>(BasicKernelGenesisConfig {
        chain_state: ChainStateConfig {
            genesis_da_height: 0,
            current_time: Default::default(),
            operating_mode: sov_chain_state::OperatingMode::Optimistic,
            inner_code_commitment: Default::default(),
            outer_code_commitment: Default::default(),
        },
    });

    const NUM_SLOTS: u64 = 10;

    last_state_root_closure(
        &mut |TestClosureArgs {
                  prev_finalize_hook_hash,
                  begin_slot_hash,
                  finalize_hook_hash,
                  current_slot_hash,
              }| {
            assert_eq!(
                prev_finalize_hook_hash, begin_slot_hash,
                "The previous finalize slot hash should match the current begin slot hash"
            );

            assert_ne!(
                finalize_hook_hash, begin_slot_hash,
                "The begin and finalize slot hashes should not match"
            );

            assert_eq!(
                finalize_hook_hash, current_slot_hash,
                "The last finalize slot hash should match the current slot hash"
            );
        },
        &mut runner,
        NUM_SLOTS,
    );
}

/// Tests that the visible kernel hash does not update for each slot for the soft confirmations Kernel
#[test]
fn visible_hash_soft_confirmations_kernel() {
    let (_, mut runner) =
        setup::<SoftConfirmationsKernel<S, MockDaSpec>>(SoftConfirmationsKernelGenesisConfig {
            chain_state: ChainStateConfig {
                genesis_da_height: 0,
                current_time: Default::default(),
                operating_mode: sov_chain_state::OperatingMode::Optimistic,
                inner_code_commitment: Default::default(),
                outer_code_commitment: Default::default(),
            },
        });

    const NUM_SLOTS: u64 = config_value!("DEFERRED_SLOTS_COUNT");

    let genesis_hash = runner
        .state_root()
        .clone()
        .namespace_root(ProvableNamespace::Kernel);

    // We run `DEFERRED_SLOTS_COUNT` slots. The visible hash should not update
    last_state_root_closure(
        &mut |TestClosureArgs {
                  prev_finalize_hook_hash,
                  begin_slot_hash,
                  finalize_hook_hash,
                  current_slot_hash,
              }| {
            assert_eq!(
                prev_finalize_hook_hash, begin_slot_hash,
                "The previous finalize slot hash should match the current begin slot hash"
            );

            assert_eq!(
                begin_slot_hash, genesis_hash,
                "The begin slot hash should match the genesis hash"
            );

            assert_eq!(
                finalize_hook_hash, begin_slot_hash,
                "The begin and finalize slot hashes should match"
            );

            assert_ne!(
                finalize_hook_hash, current_slot_hash,
                "The last finalize slot hash should not match the current slot hash"
            );
        },
        &mut runner,
        NUM_SLOTS,
    );

    // We expect that the new kernel root matches the one after the first transition (deferred update).
    let expected_visible_hash = runner.query_state(|state| {
        ChainState::<S, MockDaSpec>::default()
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
                  ..
              }| {
            assert_ne!(
                begin_slot_hash, finalize_hook_hash,
                "The last begin and finalize kernel hashes should not match"
            );

            assert_eq!(
                finalize_hook_hash, expected_visible_hash,
                "The last finalize kernel hash should match the hash of the first transition"
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
