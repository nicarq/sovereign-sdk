//! Defines RPC and REST queries exposed by the bank module, along with the relevant types.

use axum::routing::get;
use jsonrpsee::core::RpcResult;
use sov_modules_api::macros::rpc_gen;
use sov_modules_api::prelude::utoipa::openapi::OpenApi;
use sov_modules_api::prelude::{axum, serde_yaml, UnwrapInfallible};
use sov_modules_api::rest::utils::{errors, ApiResult, Path, Query};
use sov_modules_api::rest::{ApiState, HasCustomRestApi};
use sov_modules_api::ApiStateAccessor;

use crate::{get_token_id, Amount, Bank, Coins, TokenId};

/// Structure returned by the `balance_of` rpc method.
#[derive(Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, Clone)]
pub struct BalanceResponse {
    /// The balance amount of a given user for a given token. Equivalent to u64.
    pub amount: Option<Amount>,
}

/// Structure returned by the `supply_of` rpc method.
#[derive(Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, Clone)]
pub struct TotalSupplyResponse {
    /// The amount of token supply for a given token ID. Equivalent to u64.
    pub amount: Option<Amount>,
}

#[rpc_gen(client, server, namespace = "bank")]
impl<S: sov_modules_api::Spec> Bank<S> {
    #[rpc_method(name = "balanceOf")]
    /// Rpc method that returns the balance of the user at the address `user_address` for the token
    /// stored at the address `token_id`.
    pub fn balance_of(
        &self,
        version: Option<u64>,
        user_address: S::Address,
        token_id: TokenId,
        state: &mut ApiStateAccessor<S>,
    ) -> RpcResult<BalanceResponse> {
        let amount = if let Some(v) = version {
            self.get_balance_of(&user_address, token_id, &mut state.get_archival_at(v))
        } else {
            self.get_balance_of(&user_address, token_id, state)
        }
        .unwrap_infallible();
        Ok(BalanceResponse { amount })
    }

    #[rpc_method(name = "supplyOf")]
    /// Rpc method that returns the supply of a token stored at the address `token_id`.
    pub fn supply_of(
        &self,
        version: Option<u64>,
        token_id: TokenId,
        state: &mut ApiStateAccessor<S>,
    ) -> RpcResult<TotalSupplyResponse> {
        let amount = if let Some(v) = version {
            self.get_total_supply_of(&token_id, &mut state.get_archival_at(v))
        } else {
            self.get_total_supply_of(&token_id, state)
        }
        .unwrap_infallible();
        Ok(TotalSupplyResponse { amount })
    }

    #[rpc_method(name = "tokenId")]
    /// RPC method that returns the token ID for a given token name, sender, and salt.
    pub fn token_id(
        &self,
        token_name: String,
        sender: S::Address,
        salt: u64,
    ) -> RpcResult<TokenId> {
        Ok(get_token_id::<S>(&token_name, &sender, salt))
    }
}

/// Axum routes.
impl<S: sov_modules_api::Spec> Bank<S> {
    async fn route_balance(
        state: ApiState<Self, S>,
        Path((token_id, user_address)): Path<(TokenId, S::Address)>,
    ) -> ApiResult<Coins> {
        let amount = state
            .get_balance_of(&user_address, token_id, &mut state.api_state_accessor())
            .unwrap_infallible()
            .ok_or_else(|| errors::not_found_404("Balance", user_address))?;

        Ok(Coins { amount, token_id }.into())
    }

    async fn route_total_supply(
        state: ApiState<Self, S>,
        Path(token_id): Path<TokenId>,
    ) -> ApiResult<Coins> {
        let amount = state
            .get_total_supply_of(&token_id, &mut state.api_state_accessor())
            .unwrap_infallible()
            .ok_or_else(|| errors::not_found_404("Token", token_id))?;

        Ok(Coins { amount, token_id }.into())
    }

    async fn route_find_token_id(
        params: Query<types::FindTokenIdQueryParams<S::Address>>,
    ) -> ApiResult<types::TokenIdResponse> {
        let token_id = get_token_id::<S>(&params.token_name, &params.sender, params.salt);
        Ok(types::TokenIdResponse { token_id }.into())
    }

    async fn route_authorized_minters(
        state: ApiState<Self, S>,
        Path(token_id): Path<TokenId>,
    ) -> ApiResult<types::AuthorizedMintersResponse<S>> {
        let authorized_minters = state
            .tokens
            .get(&token_id, &mut state.api_state_accessor())
            .unwrap_infallible()
            .ok_or_else(|| errors::not_found_404("Token", token_id))?
            .authorized_minters;
        Ok(types::AuthorizedMintersResponse { authorized_minters }.into())
    }
}

impl<S: sov_modules_api::Spec> HasCustomRestApi for Bank<S> {
    type Spec = S;
    fn custom_rest_api(&self, state: ApiState<Self, S>) -> axum::Router<()> {
        axum::Router::new()
            .route(
                "/tokens/:tokenId/balances/:address",
                get(Self::route_balance),
            )
            .route(
                "/tokens/:tokenId/total-supply",
                get(Self::route_total_supply),
            )
            .route(
                "/tokens/:tokenId/authorized-minters",
                get(Self::route_authorized_minters),
            )
            .route("/tokens", get(Self::route_find_token_id))
            .with_state(state)
    }

    fn custom_openapi_spec(&self) -> Option<OpenApi> {
        let open_api =
            serde_yaml::from_str(include_str!("../openapi-v3.yaml")).expect("Invalid OpenAPI spec");
        Some(open_api)
    }
}

#[allow(missing_docs)]
pub mod types {
    use super::*;
    use crate::utils::TokenHolder;

    #[derive(Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, Clone)]
    pub struct FindTokenIdQueryParams<Addr> {
        pub token_name: String,
        pub sender: Addr,
        pub salt: u64,
    }

    #[derive(Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, Clone)]
    pub struct TokenIdResponse {
        pub token_id: TokenId,
    }

    #[derive(Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, Clone)]
    #[serde(bound = "S::Address: serde::Serialize + serde::de::DeserializeOwned")]
    pub struct AuthorizedMintersResponse<S: sov_modules_api::Spec> {
        pub authorized_minters: Vec<TokenHolder<S>>,
    }
}
