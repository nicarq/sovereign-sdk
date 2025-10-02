mod call;
mod event;
mod genesis;
#[cfg(feature = "native")]
mod query;

pub use call::{CallMessage, EvenPublic};
pub use event::Event;
#[cfg(feature = "native")]
pub use query::*;

use serde::{Deserialize, Serialize};
use sov_modules_api::{
    Context, DaSpec, GenesisState, Module, ModuleId, ModuleInfo, ModuleRestApi, Spec, StateValue,
    TxState,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ZkPocConfig {
    /// RISC Zero method ID (code commitment) of the even-check guest program (32 bytes)
    pub method_id: [u8; 32],
}

/// ZkPoc is a simple POC module that sets a numeric value
/// only if a valid proof is provided that the value is divisible by 2.
#[derive(Clone, ModuleInfo, ModuleRestApi)]
pub struct ZkPoc<S: Spec> {
    /// Id of the module.
    #[id]
    pub id: ModuleId,

    /// The stored numeric value.
    #[state]
    pub value: StateValue<u64>,

    /// Code commitment (32 bytes) of the guest program that verifies evenness
    #[state]
    pub method_id: StateValue<[u8; 32]>,

    /// Reference to the Bank module.
    #[module]
    pub(crate) _bank: sov_bank::Bank<S>,
}

impl<S: Spec> Module for ZkPoc<S> {
    type Spec = S;

    type Config = ZkPocConfig;

    type CallMessage = CallMessage;

    type Event = Event;

    fn genesis(
        &mut self,
        _genesis_rollup_header: &<<S as Spec>::Da as DaSpec>::BlockHeader,
        config: &Self::Config,
        state: &mut impl GenesisState<S>,
    ) -> anyhow::Result<()> {
        self.init_module(config, state)
    }

    fn call(
        &mut self,
        msg: Self::CallMessage,
        context: &Context<Self::Spec>,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<()> {
        match msg {
            CallMessage::SetValue { value, proof } =>
                Ok(self.set_value(value, proof, context, state)?),
        }
    }
}
