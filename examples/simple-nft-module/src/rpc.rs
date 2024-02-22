use jsonrpsee::core::RpcResult;
use sov_modules_api::macros::rpc_gen;
use sov_modules_api::{Spec, StateMapAccessor, WorkingSet};

use crate::NonFungibleToken;

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
/// Response for `getOwner` method
pub struct OwnerResponse<S: Spec> {
    /// Optional owner address
    pub owner: Option<S::Address>,
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
