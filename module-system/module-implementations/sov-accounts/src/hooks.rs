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
        let pub_key_hash = &tx.pub_key_hash;
        let maybe_address = self
            .accounts
            .get(pub_key_hash, state_checkpoint)
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
                        .set(pub_key_hash, &new_account, state_checkpoint);

                    self.public_keys
                        .set(default_address, pub_key_hash, state_checkpoint);

                    Ok(default_address.clone())
                }
                None => anyhow::bail!("No default address found for {}", pub_key_hash),
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
        let pub_key_hash = &tx.pub_key_hash;
        let sender_nonce = self
            .accounts
            .get(pub_key_hash, state_checkpoint)
            .map(|a| a.nonce)
            .unwrap_or_else(|| panic!("The existence of the sender account for {} is ensured during the resolution of the sender's address.",
              &pub_key_hash));

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
        let pub_key_hash = &tx.pub_key_hash;
        let mut account = self
            .accounts
            .get(pub_key_hash, state_checkpoint)
            .unwrap_or_else(|| panic!("The existence of the sender account for {} is ensured during the resolution of the sender's address.",
                &pub_key_hash));

        account.nonce += 1;

        self.accounts
            .set(&tx.pub_key_hash, &account, state_checkpoint);
    }
}
