use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{CredentialId, InfallibleStateAccessor, Spec, StateAccessor};

use crate::Uniqueness;
impl<S: Spec> Uniqueness<S> {
    pub(crate) fn check_nonce_uniqueness(
        &self,
        credential_id: &CredentialId,
        transaction_nonce: u64,
        state_checkpoint: &mut impl StateAccessor,
    ) -> anyhow::Result<()> {
        let nonce = self
            .nonces
            .get(credential_id, state_checkpoint)?
            .unwrap_or_default();

        anyhow::ensure!(
            nonce == transaction_nonce,
            "Tx bad nonce for credential id: {credential_id}, expected: {nonce}, but found: {transaction_nonce}",
        );

        Ok(())
    }

    pub(crate) fn mark_nonce_tx_attempted(
        &self,
        credential_id: &CredentialId,
        tx_scratchpad: &mut impl InfallibleStateAccessor,
    ) {
        let nonce = self
            .nonces
            .get(credential_id, tx_scratchpad)
            .unwrap_infallible()
            .unwrap_or_default();

        let nonce = nonce + 1;

        self.nonces
            .set(credential_id, &nonce, tx_scratchpad)
            .unwrap_infallible();
    }
}
