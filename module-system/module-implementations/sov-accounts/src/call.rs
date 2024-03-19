use anyhow::{ensure, Result};
use sov_modules_api::{CallResponse, Context, CryptoSpec, Signature, Spec, WorkingSet};

use crate::Accounts;

/// To update the account's public key, the sender must sign this message as proof of possession of the new key.
pub const UPDATE_ACCOUNT_MSG: [u8; 32] = [1; 32];

/// Represents the available call messages for interacting with the sov-accounts module.
#[cfg_attr(
    feature = "native",
    derive(schemars::JsonSchema),
    derive(sov_modules_api::macros::CliWalletArg),
    schemars(
        bound = "<S::CryptoSpec as CryptoSpec>::PublicKey: ::schemars::JsonSchema, <S::CryptoSpec as CryptoSpec>::Signature: ::schemars::JsonSchema",
        rename = "CallMessage"
    )
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
pub enum CallMessage<S: Spec> {
    /// Updates a public key for the corresponding Account.
    /// The sender must be in possession of the new key.
    UpdatePublicKey(
        /// The new public key
        <S::CryptoSpec as CryptoSpec>::PublicKey,
        /// A valid signature from the new public key
        <S::CryptoSpec as CryptoSpec>::Signature,
    ),
}

impl<S: Spec> Accounts<S> {
    pub(crate) fn update_public_key(
        &self,
        new_pub_key: <S::CryptoSpec as CryptoSpec>::PublicKey,
        signature: <S::CryptoSpec as CryptoSpec>::Signature,
        context: &Context<S>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<CallResponse> {
        self.exit_if_account_exists(&new_pub_key, working_set)?;

        let pub_key = self.public_keys.get_or_err(context.sender(), working_set)?;

        let account = self.accounts.remove_or_err(&pub_key, working_set)?;
        // Sanity check
        ensure!(
            context.sender() == &account.addr,
            "Inconsistent account data"
        );

        // Proof that the sender is in possession of the `new_pub_key`.
        signature.verify(&new_pub_key, &UPDATE_ACCOUNT_MSG)?;

        // Update the public key (account data remains the same).
        self.accounts.set(&new_pub_key, &account, working_set);
        self.public_keys
            .set(context.sender(), &new_pub_key, working_set);
        Ok(CallResponse::default())
    }

    fn exit_if_account_exists(
        &self,
        new_pub_key: &<S::CryptoSpec as CryptoSpec>::PublicKey,
        working_set: &mut WorkingSet<S>,
    ) -> Result<()> {
        anyhow::ensure!(
            self.accounts.get(new_pub_key, working_set).is_none(),
            "New PublicKey already exists"
        );
        Ok(())
    }
}
