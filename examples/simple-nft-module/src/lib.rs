#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

mod call;

pub use call::CallMessage;
mod genesis;
pub use genesis::*;
#[cfg(feature = "native")]
mod rpc;
#[cfg(feature = "native")]
pub use rpc::*;
use sov_modules_api::{
    CallResponse, Context, Error, Module, ModuleId, ModuleInfo, Spec, WorkingSet,
};
mod event;
pub use crate::event::Event;

#[cfg_attr(feature = "native", derive(sov_modules_api::ModuleCallJsonSchema))]
#[derive(ModuleInfo, Clone)]
/// Module for non-fungible tokens (NFT).
/// Each token is represented by a unique ID.
pub struct NonFungibleToken<S: Spec> {
    #[address]
    /// The id of the NonFungibleToken module.
    id: ModuleId,

    #[state]
    /// Admin of the NonFungibleToken module.
    admin: sov_modules_api::StateValue<S::Address>,

    #[state]
    /// Mapping of tokens to their owners.
    owners: sov_modules_api::StateMap<u64, S::Address>,

    /// Mapping from owners to their NFT ownership count.
    ///
    /// Note, this is an "accessory" state value, meaning it is not part of the
    /// proven state and should only be used for tooling, RPC, and debugging.
    /// You should only ever read and write to this value from within the
    /// `native` feature flag. See [`sov_modules_api::AccessoryStateValue`] for
    /// more information.
    #[state]
    nft_count_by_owner: sov_modules_api::AccessoryStateMap<S::Address, u64>,
}

impl<S: Spec> Module for NonFungibleToken<S> {
    type Spec = S;

    type Config = NonFungibleTokenConfig<S>;

    type CallMessage = CallMessage<S>;

    type Event = Event;

    fn genesis(&self, config: &Self::Config, working_set: &mut WorkingSet<S>) -> Result<(), Error> {
        Ok(self.init_module(config, working_set)?)
    }

    fn call(
        &self,
        msg: Self::CallMessage,
        context: &Context<Self::Spec>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<CallResponse, Error> {
        let call_result = match msg {
            CallMessage::Mint { id } => self.mint(id, context, working_set),
            CallMessage::Transfer { to, id } => self.transfer(id, to, context, working_set),
            CallMessage::Burn { id } => self.burn(id, context, working_set),
        };
        Ok(call_result?)
    }
}
