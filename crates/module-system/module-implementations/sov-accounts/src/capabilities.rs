use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{CredentialId, InfallibleStateAccessor, Spec};

use crate::{Account, Accounts};

impl<S: Spec> Accounts<S> {
    /// Resolve the sender's public key to an address.
    /// If the sender is not registered, but a fallback address if provided, immediately registers
    /// the credential to the fallback and then returns it.
    ///
    /// # Errors
    /// If the credential is not registered AND no fallback is provided, returns an error.
    pub fn resolve_sender_address(
        &self,
        default_address: &S::Address,
        credential_id: &CredentialId,
        state: &mut impl InfallibleStateAccessor,
    ) -> anyhow::Result<S::Address> {
        let maybe_address = self
            .accounts
            .get(credential_id, state)
            .unwrap_infallible()
            .map(|a| a.addr);

        match maybe_address {
            Some(address) => Ok(address),
            None => {
                // 1. Add the credential -> account mapping
                let new_account = Account {
                    addr: default_address.clone(),
                };
                self.accounts.set(credential_id, &new_account, state)?;

                Ok(default_address.clone())
            }
        }
    }
}
