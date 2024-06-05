use jsonrpsee::core::RpcResult;
use sov_modules_api::macros::rpc_gen;
use sov_modules_api::{ApiStateAccessor, Spec, StateReader};
use sov_state::User;

use crate::NonFungibleToken;

/// Response for `getOwner` method.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OwnerResponse<S: Spec> {
    /// Optional owner address
    pub owner: Option<S::Address>,
}

/// Response for `getNftsCount` method.
#[derive(Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, Clone)]
pub struct NftsCountResponse {
    /// The amount of NFTs owned by a certain address.
    pub count: u64,
}

impl<S: Spec> NonFungibleToken<S> {
    /// Get the owner of a token
    pub fn get_owner(
        &self,
        token_id: u64,
        api_state_accessor: &mut impl StateReader<User>,
    ) -> OwnerResponse<S> {
        OwnerResponse {
            owner: self.owners.get(&token_id, api_state_accessor),
        }
    }
}

#[rpc_gen(client, server, namespace = "nft")]
impl<S: Spec> NonFungibleToken<S> {
    #[rpc_method(name = "getOwner")]
    /// Get the owner of a token
    pub fn get_owner_rpc(
        &self,
        token_id: u64,
        api_state_accessor: &mut ApiStateAccessor<S>,
    ) -> RpcResult<OwnerResponse<S>> {
        Ok(self.get_owner(token_id, api_state_accessor))
    }
}
