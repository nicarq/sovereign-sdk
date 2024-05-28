use anyhow::Result;
use sov_modules_api::{CallResponse, Context, CredentialId, Spec, StateReader, TxState};
use sov_state::namespaces::User;

use crate::{Account, Accounts};

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
    /// Inserts a new credential id for the corresponding Account.
    InsertCredentialId(
        /// The new credential id.
        CredentialId,
    ),
}

impl<S: Spec> Accounts<S> {
    pub(crate) fn insert_credential_id(
        &self,
        new_credential_id: CredentialId,
        context: &Context<S>,
        working_set: &mut impl TxState<S>,
    ) -> Result<CallResponse> {
        self.exit_if_account_exists(&new_credential_id, working_set)?;

        // Insert the new credential id.
        let account = Account {
            addr: context.sender().clone(),
        };

        self.accounts.set(&new_credential_id, &account, working_set);

        let mut credential_ids = self
            .credential_ids
            .get_or_err(context.sender(), working_set)?;

        credential_ids.push(new_credential_id);
        self.credential_ids
            .set(context.sender(), &credential_ids, working_set);

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
