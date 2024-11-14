#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

mod call;

pub use call::CallMessage;
mod genesis;
pub use genesis::*;
#[cfg(feature = "native")]
mod query;
#[cfg(feature = "native")]
pub use query::*;
use sov_modules_api::{
    Context, DaSpec, Error, GenesisState, Module, ModuleId, ModuleInfo, ModuleRestApi, Spec,
    TxState,
};
mod event;
pub use crate::event::Event;

/// Module for non-fungible tokens (NFT).
/// Each token is represented by a unique ID.
#[derive(Clone, ModuleInfo, ModuleRestApi)]
pub struct NonFungibleToken<S: Spec> {
    #[id]
    /// The id of the NonFungibleToken module.
    id: ModuleId,

    #[state]
    /// Admin of the NonFungibleToken module.
    admin: sov_modules_api::StateValue<S::Address>,

    #[state]
    /// Mapping of tokens to their owners.
    owners: sov_modules_api::StateMap<u64, S::Address>,
}

impl<S: Spec> Module for NonFungibleToken<S> {
    type Spec = S;

    type Config = NonFungibleTokenConfig<S>;

    type CallMessage = CallMessage<S>;

    type Event = Event;

    fn genesis(
        &self,
        _genesis_rollup_header: &<<S as Spec>::Da as DaSpec>::BlockHeader,
        _validity_condition: &<<S as Spec>::Da as DaSpec>::ValidityCondition,
        config: &Self::Config,
        state: &mut impl GenesisState<S>,
    ) -> Result<(), Error> {
        Ok(self.init_module(config, state)?)
    }

    fn call(
        &self,
        msg: Self::CallMessage,
        context: &Context<Self::Spec>,
        state: &mut impl TxState<S>,
    ) -> Result<(), Error> {
        let call_result = match msg {
            CallMessage::Mint { id } => self.mint(id, context, state),
            CallMessage::Transfer { to, id } => self.transfer(id, to, context, state),
            CallMessage::Burn { id } => self.burn(id, context, state),
        };
        Ok(call_result?)
    }
}
