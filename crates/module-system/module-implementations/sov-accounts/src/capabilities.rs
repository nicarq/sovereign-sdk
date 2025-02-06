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
        maybe_fallback_address: &Option<S::Address>,
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
            None => match maybe_fallback_address {
                Some(default_address) => {
                    // 1. Add the credential -> account mapping
                    let new_account = Account {
                        addr: default_address.clone(),
                    };
                    self.accounts.set(credential_id, &new_account, state)?;

                    // 2. Append to, or create, the account -> credential mapping
                    let mut credential_ids = self
                        .credential_ids
                        .get(default_address, state)
                        .unwrap_infallible()
                        .unwrap_or_default();

                    credential_ids.push(*credential_id);

                    self.credential_ids
                        .set(default_address, &credential_ids, state)?;

                    Ok(default_address.clone())
                }
                None => anyhow::bail!(
                    "No account found for {}, and no default address was provided",
                    credential_id
                ),
            },
        }
    }
}
