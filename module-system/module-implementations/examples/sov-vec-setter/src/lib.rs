#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
mod call;
mod event;
mod genesis;

#[cfg(feature = "native")]
mod rpc;

pub use call::CallMessage;
#[cfg(feature = "native")]
pub use rpc::*;
use serde::{Deserialize, Serialize};
use sov_modules_api::{Context, Error, ModuleInfo, WorkingSet};

use crate::event::Event;

/// Initial configuration for sov-vec-setter module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VecSetterConfig<S: sov_modules_api::Spec> {
    /// Admin of the module.
    pub admin: S::Address,
}

/// A new module:
/// - Must derive `ModuleInfo`
/// - Must contain `[address]` field
/// - Can contain any number of ` #[state]` or `[module]` fields
#[cfg_attr(feature = "native", derive(sov_modules_api::ModuleCallJsonSchema))]
#[derive(ModuleInfo)]
pub struct VecSetter<S: sov_modules_api::Spec> {
    /// Address of the module.
    #[address]
    pub address: S::Address,

    /// Some vector kept in the state.
    #[state]
    pub vector: sov_modules_api::StateVec<u32>,

    /// Holds the address of the admin user who is allowed to update the vector.
    #[state]
    pub admin: sov_modules_api::StateValue<S::Address>,
}

impl<S: sov_modules_api::Spec> sov_modules_api::Module for VecSetter<S> {
    type Spec = S;

    type Config = VecSetterConfig<S>;

    type CallMessage = call::CallMessage;

    type Event = Event;

    fn genesis(&self, config: &Self::Config, working_set: &mut WorkingSet<S>) -> Result<(), Error> {
        // The initialization logic
        Ok(self.init_module(config, working_set)?)
    }

    fn call(
        &self,
        msg: Self::CallMessage,
        context: &Context<Self::Spec>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<sov_modules_api::CallResponse, Error> {
        match msg {
            call::CallMessage::PushValue(new_value) => {
                Ok(self.push_value(new_value, context, working_set)?)
            }
            call::CallMessage::SetValue { index, value } => {
                Ok(self.set_value(index, value, context, working_set)?)
            }
            call::CallMessage::SetAllValues(values) => {
                Ok(self.set_all_values(values, context, working_set)?)
            }
            call::CallMessage::PopValue => Ok(self.pop_value(context, working_set)?),
        }
    }
}
