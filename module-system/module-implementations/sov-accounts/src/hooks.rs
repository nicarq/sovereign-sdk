use sov_modules_api::transaction::AuthenticatedTransactionData;
use sov_modules_api::{Hash, Spec, StateAccessor, StateCheckpoint};

use crate::{Account, Accounts};

impl<S: Spec> Accounts<S> {
    pub(crate) fn get_or_create_default(
        &self,
        pub_key_hash: &Hash,
        default_address: &S::Address,
        working_set: &mut impl StateAccessor,
    ) -> Account<S> where {
        if let Some(acct) = self.accounts.get(pub_key_hash, working_set) {
            acct
        } else {
            let new_account = Account {
                addr: default_address.clone(),
                nonce: 0,
            };

            self.accounts.set(pub_key_hash, &new_account, working_set);

            self.public_keys
                .set(default_address, pub_key_hash, working_set);
            new_account
        }
    }

    /// Checks that a transaction is not a duplicate
    // TODO(@preston-evans98): Enforce that this is read-only
    pub fn check_uniqueness(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> Result<(), anyhow::Error> {
        // TODO(@preston-evans98) - this check should rely on the information resolved from the context.
        // This will require a change to the account state layout

        let sender_nonce = self
            .accounts
            .get(tx.pub_key_hash(), state_checkpoint)
            .map(|a| a.nonce)
            .unwrap_or(0);
        let tx_nonce = tx.nonce();

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
        // TODO(@preston-evans98) - this check should rely on the information resolved from the context.
        // This will require a change to the account state layout
        let mut account =
            self.get_or_create_default(tx.pub_key_hash(), tx.default_address(), state_checkpoint);
        account.nonce += 1;

        self.accounts
            .set(tx.pub_key_hash(), &account, state_checkpoint);
    }

    /// Resolve the sender public key to an address
    pub fn resolve_sender_address(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> S::Address {
        self.accounts
            .get(tx.pub_key_hash(), state_checkpoint)
            .map(|a| a.addr)
            .unwrap_or(tx.default_address().clone())
    }
}
