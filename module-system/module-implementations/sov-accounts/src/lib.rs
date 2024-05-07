#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
mod call;
#[cfg(all(feature = "arbitrary", feature = "native"))]
mod fuzz;
mod genesis;
mod hooks;
pub use genesis::*;
#[cfg(feature = "native")]
mod rpc;
#[cfg(feature = "native")]
pub use rpc::*;
mod event;
#[cfg(test)]
mod tests;
pub use call::CallMessage;
use sov_modules_api::{Context, CryptoSpec, Error, Hash, ModuleId, ModuleInfo, Spec, WorkingSet};
use sov_state::storage::TxState;

use crate::event::Event;

impl<S: Spec> FromIterator<<S::CryptoSpec as CryptoSpec>::PublicKey> for AccountConfig<S> {
    fn from_iter<T: IntoIterator<Item = <S::CryptoSpec as CryptoSpec>::PublicKey>>(
        iter: T,
    ) -> Self {
        Self {
            pub_keys: iter.into_iter().collect(),
        }
    }
}

/// An account on the rollup.
#[derive(borsh::BorshDeserialize, borsh::BorshSerialize, Debug, PartialEq, Copy, Clone)]
pub struct Account<S: Spec> {
    /// The address of the account.
    pub addr: S::Address,
    /// The current nonce value associated with the account.
    pub nonce: u64,
}

/// A module responsible for managing accounts on the rollup.
#[cfg_attr(feature = "native", derive(sov_modules_api::ModuleCallJsonSchema))]
#[derive(ModuleInfo, Clone)]
#[cfg_attr(feature = "arbitrary", derive(Debug))]
pub struct Accounts<S: Spec> {
    /// The ID of the sov-accounts module.
    #[id]
    pub id: ModuleId,

    /// Mapping from an account address to a corresponding public key.
    #[state]
    pub(crate) public_keys: sov_modules_api::StateMap<S::Address, Hash>,

    /// Mapping from a public key to a corresponding account.
    #[state]
    pub(crate) accounts: sov_modules_api::StateMap<Hash, Account<S>>,
}

impl<S: Spec> sov_modules_api::Module for Accounts<S> {
    type Spec = S;

    type Config = AccountConfig<S>;

    type CallMessage = call::CallMessage;

    type Event = Event;

    fn genesis(&self, config: &Self::Config, working_set: &mut WorkingSet<S>) -> Result<(), Error> {
        Ok(self.init_module(config, working_set)?)
    }

    fn call(
        &self,
        msg: Self::CallMessage,
        context: &Context<S>,
        working_set: &mut impl TxState<S>,
    ) -> Result<sov_modules_api::CallResponse, Error> {
        match msg {
            call::CallMessage::UpdatePublicKey(new_pub_key_hash) => {
                Ok(self.update_public_key(new_pub_key_hash, context, working_set)?)
            }
        }
    }
}
