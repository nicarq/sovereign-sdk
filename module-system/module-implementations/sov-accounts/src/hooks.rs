use sov_modules_api::transaction::AuthenticatedTransactionData;
use sov_modules_api::{Spec, StateCheckpoint};

use crate::{Account, Accounts};

impl<S: Spec> Accounts<S> {
    /// Resolve the sender's public key to an address. Return an error if the sender is not registered.
    pub fn resolve_sender_address(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> Result<S::Address, anyhow::Error> {
        let credential_id = &tx.credential_id;
        let maybe_address = self
            .accounts
            .get(credential_id, state_checkpoint)
            .map(|a| a.addr);

        match maybe_address {
            Some(address) => Ok(address),
            None => match &tx.default_address {
                Some(default_address) => {
                    let new_account = Account {
                        addr: default_address.clone(),
                        nonce: 0,
                    };

                    self.accounts
                        .set(credential_id, &new_account, state_checkpoint);

                    self.credential_ids.set(
                        default_address,
                        &vec![*credential_id],
                        state_checkpoint,
                    );

                    Ok(default_address.clone())
                }
                None => anyhow::bail!("No default address found for {}", credential_id),
            },
        }
    }

    /// Checks that a transaction is not a duplicate
    pub fn check_uniqueness(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> Result<(), anyhow::Error> {
        // TODO(@preston-evans98) - this check should rely on the information resolved from the context.
        // This will require a change to the account state layout
        let credential_id = &tx.credential_id;
        let sender_nonce = self
            .accounts
            .get(credential_id, state_checkpoint)
            .map(|a| a.nonce)
            .unwrap_or_else(|| panic!("The existence of the sender account for {} is ensured during the resolution of the sender's address.",
              &credential_id));

        let tx_nonce = tx.nonce;

        anyhow::ensure!(
            sender_nonce == tx_nonce,
            "Tx bad nonce, expected: {}, but found: {}",
            tx_nonce,
            sender_nonce
        );
        Ok(())
    }

    /// Marks a transaction as attempted, ensuring that future attempts at execution will fail
    pub fn mark_tx_attempted(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        let credential_id = &tx.credential_id;
        let mut account = self
            .accounts
            .get(credential_id, state_checkpoint)
            .unwrap_or_else(|| panic!("The existence of the sender account for {} is ensured during the resolution of the sender's address.",
                &credential_id));

        account.nonce += 1;

        self.accounts
            .set(&tx.credential_id, &account, state_checkpoint);
    }
}
