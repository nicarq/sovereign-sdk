use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{
    Context, DaSpec, GenesisState, Module, ModuleError, ModuleId, ModuleInfo, SlotHooks, Spec,
    StateCheckpoint, StateVec, TxState,
};
use sov_state::Storage;
use sov_test_utils::generate_zk_runtime;
use sov_test_utils::runtime::genesis::zk::config::HighLevelZkGenesisConfig;

use crate::hooks_derive::TestRunner;

type S = super::S;

#[derive(ModuleInfo, Clone)]
pub struct IncorrectHooksOverride<S: Spec> {
    #[id]
    id: ModuleId,

    #[state]
    begin_slot_hash: StateVec<<S::Storage as Storage>::Root>,
}

impl<S: Spec> Module for IncorrectHooksOverride<S> {
    type Spec = S;
    type Config = ();
    type CallMessage = ();
    type Event = ();

    fn genesis(
        &self,
        _genesis_rollup_header: &<S::Da as DaSpec>::BlockHeader,
        _validity_condition: &<S::Da as DaSpec>::ValidityCondition,
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
    ) -> Result<(), ModuleError> {
        Ok(())
    }
}

generate_zk_runtime!(IncorrectHooksRuntime <= hooks: IncorrectHooksOverride<S>);

type RT = IncorrectHooksRuntime<S>;

fn setup() -> TestRunner<RT> {
    let genesis_config = HighLevelZkGenesisConfig::generate();

    let genesis = GenesisConfig::from_minimal_config(genesis_config.into(), ());

    TestRunner::new_with_genesis(genesis.into_genesis_params(), RT::default())
}

impl SlotHooks for IncorrectHooksOverride<S> {
    type Spec = S;

    fn begin_slot_hook(
        &self,
        visible_hash: &<<S as Spec>::Storage as Storage>::Root,
        state: &mut StateCheckpoint<<Self::Spec as Spec>::Storage>,
    ) {
        self.begin_slot_hash
            .push(visible_hash, state)
            .unwrap_infallible();
    }
}

/// The hook override fails if the module does not implement the [`SlotHooks`] trait for every spec.
#[test]
fn hook_override_fails_when_not_generic_override() {
    let mut runner = setup();

    runner.advance_slots(1);

    runner.query_state(|state| {
        assert_eq!(
            IncorrectHooksOverride::<S>::default()
                .begin_slot_hash
                .len(state)
                .unwrap_infallible(),
            0,
            "The hooks should not have ran"
        );
    });
}
