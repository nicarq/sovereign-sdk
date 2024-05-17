use anyhow::{ensure, Result};
use sov_modules_api::{CallResponse, Context, CredentialId, Spec, TxState};
use sov_state::namespaces::User;
use sov_state::StateReader;

use crate::Accounts;

/// Represents the available call messages for interacting with the sov-accounts module.
#[cfg_attr(
    feature = "native",
    derive(schemars::JsonSchema),
    derive(sov_modules_api::macros::CliWalletArg)
)]
#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
    Debug,
    PartialEq,
    Clone,
)]
pub enum CallMessage {
    /// Updates credential id for the corresponding Account.
    UpdatePublicKey(
        /// The new credential id.
        CredentialId,
    ),
}

impl<S: Spec> Accounts<S> {
    pub(crate) fn update_public_key(
        &self,
        new_credential_id: CredentialId,
        context: &Context<S>,
        working_set: &mut impl TxState<S>,
    ) -> Result<CallResponse> {
        self.exit_if_account_exists(&new_credential_id, working_set)?;

        let credential_id = self
            .credential_ids
            .get_or_err(context.sender(), working_set)?;

        let account = self.accounts.remove_or_err(&credential_id, working_set)?;
        // Sanity check
        ensure!(
            context.sender() == &account.addr,
            "Inconsistent account data"
        );

        // Update credentials (account data remains the same).
        self.accounts.set(&new_credential_id, &account, working_set);
        self.credential_ids
            .set(context.sender(), &new_credential_id, working_set);
        Ok(CallResponse::default())
    }

    fn exit_if_account_exists(
        &self,
        new_credential_id: &CredentialId,
        working_set: &mut impl StateReader<User>,
    ) -> Result<()> {
        anyhow::ensure!(
            self.accounts.get(new_credential_id, working_set).is_none(),
            "New CredentialId already exists"
        );
        Ok(())
    }
}
