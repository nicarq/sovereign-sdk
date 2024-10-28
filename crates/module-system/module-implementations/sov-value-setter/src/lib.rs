#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
mod call;
mod genesis;

mod event;
pub use call::*;
pub use event::Event;
pub use genesis::*;
use sov_modules_api::{
    CallResponse, Context, DaSpec, Error, GenesisState, Module, ModuleId, ModuleInfo,
    ModuleRestApi, Spec, StateValue, StateVec, TxState,
};

/// A new module:
/// - Must derive `ModuleInfo`
/// - Must contain `[id]` field
/// - Can contain any number of ` #[state]` or `[module]` fields
/// - Can derive ModuleRestApi to automatically generate Rest API endpoints
#[derive(Clone, ModuleInfo, ModuleRestApi)]
pub struct ValueSetter<S: Spec> {
    /// The ID of the module.
    #[id]
    pub id: ModuleId,

    /// Some value kept in the state.
    #[state]
    pub value: StateValue<u32>,

    /// Some more values kept in state.
    #[state]
    many_values: StateVec<u8>,

    /// Holds the address of the admin user who is allowed to update the value.
    #[state]
    pub admin: StateValue<S::Address>,
}

impl<S: Spec> Module for ValueSetter<S> {
    type Spec = S;

    type Config = ValueSetterConfig<S>;

    type CallMessage = call::CallMessage;

    type Event = Event;

    fn genesis(
        &self,
        _genesis_rollup_header: &<<S as Spec>::Da as DaSpec>::BlockHeader,
        _validity_condition: &<<S as Spec>::Da as DaSpec>::ValidityCondition,
        config: &Self::Config,
        state: &mut impl GenesisState<S>,
    ) -> Result<(), Error> {
        // The initialization logic
        Ok(self.init_module(config, state)?)
    }

    fn call(
        &self,
        msg: Self::CallMessage,
        context: &Context<Self::Spec>,
        state: &mut impl TxState<S>,
    ) -> Result<CallResponse, Error> {
        match msg {
            call::CallMessage::SetValue(new_value) => {
                Ok(self.set_value(new_value, context, state)?)
            }
            CallMessage::SetManyValues(many) => Ok(self.set_values(many, context, state)?),
        }
    }
}
