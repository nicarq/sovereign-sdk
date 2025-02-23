#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
mod call;
mod capability;
pub mod derived_holder;
#[cfg(feature = "test-utils")]
mod test_utils;

pub use capability::ReserveGasError;
mod genesis;
#[cfg(feature = "native")]
mod query;
#[cfg(feature = "native")]
pub use query::*;
mod token;
/// Util functions for bank
pub mod utils;
pub use call::*;
pub use genesis::*;
use sov_modules_api::macros::config_value;
use sov_modules_api::{
    Context, DaSpec, Error, Gas, GenesisState, Module, ModuleId, ModuleInfo, ModuleRestApi, Spec,
    StateMap, TxState,
};
use sov_state::BorshCodec;
/// Specifies an interface to interact with tokens.
pub use token::{Amount, BurnRate, Coins, TokenId, TokenIdBech32};
use token::{BalanceKey, Token};
/// Methods to get a token ID.
pub use utils::{get_token_id, IntoPayable, Payable};
use utils::{TokenHolder, TokenHolderRef};

/// Event definition from module exported
/// This can be useful for deserialization from API and similar cases
pub mod event;
use crate::event::Event;

/// The [`TokenId`] of the rollup's gas token.
pub fn config_gas_token_id() -> TokenId {
    config_value!("GAS_TOKEN_ID")
}

pub(crate) type C = BorshCodec;

/// Gas configuration for the bank module
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Hash)]
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
#[derive(Clone, ModuleInfo, ModuleRestApi)]
pub struct Bank<S: Spec> {
    /// The id of the sov-bank module.
    #[id]
    pub id: ModuleId,

    /// The gas configuration of the sov-bank module.
    #[gas]
    pub(crate) gas: BankGasConfig<S::Gas>,

    /// A mapping of [`TokenId`]s to tokens in the sov-bank.
    #[state]
    pub(crate) tokens: StateMap<TokenId, Token<S>, C>,

    /// A mapping from [`TokenHolder`] and[`TokenId`] to balance in the sov-bank.
    #[state]
    pub(crate) balances: StateMap<BalanceKey<TokenHolder<S>>, Amount, C>,
}

impl<S: Spec> Module for Bank<S> {
    type Spec = S;

    type Config = BankConfig<S>;

    type CallMessage = call::CallMessage<S>;

    type Event = Event<S>;

    fn genesis(
        &self,
        _genesis_rollup_header: &<<S as Spec>::Da as DaSpec>::BlockHeader,
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
        match msg {
            call::CallMessage::CreateToken {
                token_name,
                initial_balance,
                mint_to_address,
                supply_cap,
                admins,
            } => {
                self.charge_gas(state, &self.gas.create_token)?;

                let admins = admins
                    .iter()
                    .map(|minter| TokenHolderRef::from(&minter))
                    .collect::<Vec<_>>();

                self.create_token(
                    token_name.into(),
                    initial_balance,
                    &mint_to_address,
                    admins,
                    supply_cap,
                    context.sender(),
                    state,
                )?;
                Ok(())
            }

            call::CallMessage::Transfer { to, coins } => {
                self.charge_gas(state, &self.gas.transfer)?;
                Ok(self.transfer(&to, coins, context, state)?)
            }

            call::CallMessage::Burn { coins } => {
                self.charge_gas(state, &self.gas.burn)?;
                Ok(self.burn_from_eoa(coins, context, state)?)
            }

            call::CallMessage::Mint {
                coins,
                mint_to_address,
            } => {
                self.charge_gas(state, &self.gas.mint)?;
                self.mint_from_eoa(coins, &mint_to_address, context, state)?;
                Ok(())
            }

            call::CallMessage::Freeze { token_id } => {
                self.charge_gas(state, &self.gas.freeze)?;
                Ok(self.freeze(token_id, context, state)?)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::env;

    use super::*;

    #[test]
    fn custom_gas_token_id() {
        env::set_var(
            "SOV_SDK_CONST_OVERRIDE_GAS_TOKEN_ID",
            "token_1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqnfxkwm",
        );
        assert_eq!(
            config_gas_token_id().to_string(),
            "token_1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqnfxkwm"
        );
    }
}
