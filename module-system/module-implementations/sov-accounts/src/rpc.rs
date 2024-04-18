//! Defines rpc queries exposed by the accounts module, along with the relevant types
use jsonrpsee::core::RpcResult;
use sov_modules_api::macros::rpc_gen;
use sov_modules_api::{CryptoSpec, PublicKey, Spec, WorkingSet};

use crate::{Account, Accounts};

/// This is the response returned from the accounts_getAccount endpoint.
#[derive(Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, Clone)]
#[serde(bound = "Addr: serde::Serialize + serde::de::DeserializeOwned")]
pub enum Response<Addr> {
    /// The account corresponding to the given public key exists.
    AccountExists {
        /// The address of the account,
        addr: Addr,
        /// The nonce of the account.
        nonce: u64,
    },
    /// The account corresponding to the given public key does not exist.
    AccountEmpty,
}

#[rpc_gen(client, server, namespace = "accounts")]
impl<S: Spec> Accounts<S> {
    #[rpc_method(name = "getAccount")]
    /// Get the account corresponding to the given public key.
    pub fn get_account(
        &self,
        pub_key: <S::CryptoSpec as CryptoSpec>::PublicKey,
        working_set: &mut WorkingSet<S>,
    ) -> RpcResult<Response<S::Address>> {
        let pub_key_hash = pub_key.secure_hash::<<S::CryptoSpec as CryptoSpec>::Hasher>();
        let response = match self.accounts.get(&pub_key_hash, working_set) {
            Some(Account { addr, nonce }) => Response::AccountExists { addr, nonce },
            None => Response::AccountEmpty,
        };

        Ok(response)
    }
}
