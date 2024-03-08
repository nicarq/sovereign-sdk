//! Defines rpc queries exposed by the bank module, along with the relevant types
use jsonrpsee::core::RpcResult;
use sov_modules_api::macros::rpc_gen;
use sov_modules_api::WorkingSet;

use crate::{get_token_address, Amount, Bank};

/// Structure returned by the `balance_of` rpc method.
#[derive(Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, Clone)]
pub struct BalanceResponse {
    /// The balance amount of a given user for a given token. Equivalent to u64.
    pub amount: Option<Amount>,
}

/// Structure returned by the `supply_of` rpc method.
#[derive(Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, Clone)]
pub struct TotalSupplyResponse {
    /// The amount of token supply for a given token address. Equivalent to u64.
    pub amount: Option<Amount>,
}

#[rpc_gen(client, server, namespace = "bank")]
impl<S: sov_modules_api::Spec> Bank<S> {
    #[rpc_method(name = "balanceOf")]
    /// Rpc method that returns the balance of the user at the address `user_address` for the token
    /// stored at the address `token_address`.
    pub fn balance_of(
        &self,
        version: Option<u64>,
        user_address: S::Address,
        token_address: S::Address,
        working_set: &mut WorkingSet<S>,
    ) -> RpcResult<BalanceResponse> {
        let amount = if let Some(v) = version {
            self.get_balance_of(
                user_address,
                token_address,
                &mut working_set.get_archival_at(v),
            )
        } else {
            self.get_balance_of(user_address, token_address, working_set)
        };
        Ok(BalanceResponse { amount })
    }

    #[rpc_method(name = "supplyOf")]
    /// Rpc method that returns the supply of a token stored at the address `token_address`.
    pub fn supply_of(
        &self,
        version: Option<u64>,
        token_address: S::Address,
        working_set: &mut WorkingSet<S>,
    ) -> RpcResult<TotalSupplyResponse> {
        let amount = if let Some(v) = version {
            self.get_total_supply_of(&token_address, &mut working_set.get_archival_at(v))
        } else {
            self.get_total_supply_of(&token_address, working_set)
        };
        Ok(TotalSupplyResponse { amount })
    }

    #[rpc_method(name = "tokenAddress")]
    /// RPC method that returns the token address for a given token name, sender, and salt.
    pub fn token_address(
        &self,
        token_name: String,
        sender: S::Address,
        salt: u64,
    ) -> RpcResult<S::Address> {
        Ok(get_token_address::<S>(&token_name, &sender, salt))
    }
}
