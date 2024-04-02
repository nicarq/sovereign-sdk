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
pub use call::{CallMessage, UPDATE_ACCOUNT_MSG};
use sov_modules_api::{Context, CryptoSpec, Error, ModuleId, ModuleInfo, Spec, WorkingSet};

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
    /// The address of the sov-accounts module.
    #[address]
    pub id: ModuleId,

    /// Mapping from an account address to a corresponding public key.
    #[state]
    pub(crate) public_keys:
        sov_modules_api::StateMap<S::Address, <S::CryptoSpec as CryptoSpec>::PublicKey>,

    /// Mapping from a public key to a corresponding account.
    #[state]
    pub(crate) accounts:
        sov_modules_api::StateMap<<S::CryptoSpec as CryptoSpec>::PublicKey, Account<S>>,
}

impl<S: Spec> sov_modules_api::Module for Accounts<S> {
    type Spec = S;

    type Config = AccountConfig<S>;

    type CallMessage = call::CallMessage<S>;

    type Event = Event;

    fn genesis(&self, config: &Self::Config, working_set: &mut WorkingSet<S>) -> Result<(), Error> {
        Ok(self.init_module(config, working_set)?)
    }

    fn call(
        &self,
        msg: Self::CallMessage,
        context: &Context<S>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<sov_modules_api::CallResponse, Error> {
        match msg {
            call::CallMessage::UpdatePublicKey(new_pub_key, sig) => {
                Ok(self.update_public_key(new_pub_key, sig, context, working_set)?)
            }
        }
    }
}
