mod call;
mod event;
mod genesis;
#[cfg(feature = "native")]
mod rpc;
pub use call::CallMessage;
pub use event::Event;
#[cfg(feature = "native")]
pub use rpc::*;
use serde::{Deserialize, Serialize};
use sov_modules_api::{Context, Error, ModuleId, ModuleInfo, TxState, WorkingSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExampleModuleConfig {}

/// A new module:
/// - Must derive `ModuleInfo`
/// - Must contain `[address]` field
/// - Can contain any number of ` #[state]` or `[module]` fields
/// - Should derive `ModuleCallJsonSchema` if the "native" feature is enabled.
///   This is optional, and is only used to generate a JSON Schema for your
///   module's call messages (which is useful to develop clients, CLI tooling
///   etc.).
#[cfg_attr(feature = "native", derive(sov_modules_api::ModuleCallJsonSchema))]
#[derive(ModuleInfo)]
pub struct ExampleModule<S: sov_modules_api::Spec> {
    /// Id of the module.
    #[id]
    pub id: ModuleId,

    /// Some value kept in the state.
    #[state]
    pub value: sov_modules_api::StateValue<u32>,

    /// Reference to the Bank module.
    #[module]
    pub(crate) _bank: sov_bank::Bank<S>,
}

impl<S: sov_modules_api::Spec> sov_modules_api::Module for ExampleModule<S> {
    type Spec = S;

    type Config = ExampleModuleConfig;

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
        working_set: &mut impl TxState<S>,
    ) -> Result<sov_modules_api::CallResponse, Error> {
        match msg {
            call::CallMessage::SetValue(new_value) => {
                Ok(self.set_value(new_value, context, working_set)?)
            }
        }
    }
}
