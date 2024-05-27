use jsonrpsee::core::RpcResult;
use sov_modules_api::macros::rpc_gen;
use sov_modules_api::{Spec, WorkingSet};

use crate::{CredentialId, Nonces};

/// This is the response returned from the nonces_getNonce endpoint.
#[derive(Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, Clone)]
pub struct Response {
    /// Nonce
    pub nonce: u64,
}

#[rpc_gen(client, server, namespace = "nonces")]
impl<S: Spec> Nonces<S> {
    #[rpc_method(name = "getNonce")]
    /// Get the nonce corresponding to the given credential id.
    pub fn get_nonce(
        &self,
        credential_id: CredentialId,
        working_set: &mut WorkingSet<S>,
    ) -> RpcResult<Response> {
        let nonce = self.nonce(&credential_id, working_set).unwrap_or_default();

        Ok(Response { nonce })
    }
}
