#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
mod call;
mod capability;
#[cfg(feature = "test-utils")]
mod test_utils;
pub use capability::ReserveGasError;
mod genesis;
#[cfg(feature = "native")]
mod rpc;
#[cfg(feature = "native")]
pub use rpc::*;
mod token;
/// Util functions for bank
pub mod utils;
pub use call::*;
pub use genesis::*;
use sov_modules_api::macros::config_bech32;
use sov_modules_api::{CallResponse, Context, Error, Gas, ModuleId, ModuleInfo, WorkingSet};
use token::Token;
/// Specifies an interface to interact with tokens.
pub use token::{Amount, BurnRate, Coins, TokenId, TokenIdBech32};
use utils::TokenHolderRef;
/// Methods to get a token ID.
pub use utils::{get_token_id, IntoPayable, Payable};

/// Event definition from module exported
/// This can be useful for deserialization from RPC and similar cases
pub mod event;
use crate::event::Event;

/// The [`TokenId`] of the rollup's gas token.
pub const GAS_TOKEN_ID: TokenId = config_bech32!("GAS_TOKEN_ID", TokenId);

/// Gas configuration for the bank module
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BankGasConfig<GU: Gas> {
    /// Gas price multiplier for the create token operation
    pub create_token: GU,

    /// Gas price multiplier for the transfer operation
    pub transfer: GU,

    /// Gas price multiplier for the burn operation
    pub burn: GU,

    /// Gas price multiplier for the mint operation
    pub mint: GU,

    /// Gas price multiplier for the freeze operation
    pub freeze: GU,
}

/// The sov-bank module manages user balances. It provides functionality for:
/// - Token creation.
/// - Token transfers.
/// - Token burn.
#[cfg_attr(feature = "native", derive(sov_modules_api::ModuleCallJsonSchema))]
#[derive(ModuleInfo, Clone)]
pub struct Bank<S: sov_modules_api::Spec> {
    /// The id of the sov-bank module.
    #[id]
    pub(crate) id: ModuleId,

    /// The gas configuration of the sov-bank module.
    #[gas]
    pub(crate) gas: BankGasConfig<S::Gas>,

    /// A mapping of [`TokenId`]s to tokens in the sov-bank.
    #[state]
    pub(crate) tokens: sov_modules_api::StateMap<TokenId, Token<S>>,
}

impl<S: sov_modules_api::Spec> sov_modules_api::Module for Bank<S> {
    type Spec = S;

    type Config = BankConfig<S>;

    type CallMessage = call::CallMessage<S>;

    type Event = Event;

    fn genesis(&self, config: &Self::Config, working_set: &mut WorkingSet<S>) -> Result<(), Error> {
        Ok(self.init_module(config, working_set)?)
    }

    fn call(
        &self,
        msg: Self::CallMessage,
        context: &Context<Self::Spec>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<sov_modules_api::CallResponse, Error> {
        match msg {
            call::CallMessage::CreateToken {
                salt,
                token_name,
                initial_balance,
                minter_address,
                authorized_minters,
            } => {
                self.charge_gas(working_set, &self.gas.create_token)?;

                let authorized_minters = authorized_minters
                    .iter()
                    .map(|minter| TokenHolderRef::from(&minter))
                    .collect::<Vec<_>>();

                self.create_token(
                    token_name,
                    salt,
                    initial_balance,
                    &minter_address,
                    authorized_minters,
                    context.sender(),
                    working_set,
                )?;
                Ok(CallResponse::default())
            }

            call::CallMessage::Transfer { to, coins } => {
                self.charge_gas(working_set, &self.gas.create_token)?;
                Ok(self.transfer(&to, coins, context, working_set)?)
            }

            call::CallMessage::Burn { coins } => {
                self.charge_gas(working_set, &self.gas.burn)?;
                Ok(self.burn_from_eoa(coins, context, working_set)?)
            }

            call::CallMessage::Mint {
                coins,
                minter_address,
            } => {
                self.charge_gas(working_set, &self.gas.mint)?;
                self.mint_from_eoa(&coins, &minter_address, context, working_set)?;
                Ok(CallResponse::default())
            }

            call::CallMessage::Freeze { token_id } => {
                self.charge_gas(working_set, &self.gas.freeze)?;
                Ok(self.freeze(token_id, context, working_set)?)
            }
        }
    }
}
