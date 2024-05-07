use jsonrpsee::core::RpcResult;
use sov_modules_api::macros::rpc_gen;
use sov_modules_api::{Spec, WorkingSet};

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

#[rpc_gen(client, server, namespace = "nft")]
impl<S: Spec> NonFungibleToken<S> {
    #[rpc_method(name = "getOwner")]
    /// Get the owner of a token
    pub fn get_owner(
        &self,
        token_id: u64,
        working_set: &mut WorkingSet<S>,
    ) -> RpcResult<OwnerResponse<S>> {
        Ok(OwnerResponse {
            owner: self.owners.get(&token_id, working_set),
        })
    }
}
