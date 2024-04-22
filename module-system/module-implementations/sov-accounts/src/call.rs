use anyhow::{ensure, Result};
use sov_modules_api::{CallResponse, Context, Hash, Spec, WorkingSet};

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
    /// Updates a public key for the corresponding Account.
    UpdatePublicKey(
        /// The new public key hash
        Hash,
    ),
}

impl<S: Spec> Accounts<S> {
    pub(crate) fn update_public_key(
        &self,
        new_pub_key_hash: Hash,
        context: &Context<S>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<CallResponse> {
        self.exit_if_account_exists(&new_pub_key_hash, working_set)?;

        let pub_key_hash = self.public_keys.get_or_err(context.sender(), working_set)?;

        let account = self.accounts.remove_or_err(&pub_key_hash, working_set)?;
        // Sanity check
        ensure!(
            context.sender() == &account.addr,
            "Inconsistent account data"
        );

        // Update the public key (account data remains the same).
        self.accounts.set(&new_pub_key_hash, &account, working_set);
        self.public_keys
            .set(context.sender(), &new_pub_key_hash, working_set);
        Ok(CallResponse::default())
    }

    fn exit_if_account_exists(
        &self,
        new_pub_key_hash: &Hash,
        working_set: &mut WorkingSet<S>,
    ) -> Result<()> {
        anyhow::ensure!(
            self.accounts.get(new_pub_key_hash, working_set).is_none(),
            "New PublicKey already exists"
        );
        Ok(())
    }
}
