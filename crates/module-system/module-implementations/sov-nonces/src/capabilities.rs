use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{CredentialId, Spec, StateAccessor, TxScratchpad};

use crate::Nonces;

impl<S: Spec> Nonces<S> {
    /// Checks the provided nonce.
    pub fn check_nonce(
        &self,
        credential_id: &CredentialId,
        nonce: u64,
        state_checkpoint: &mut impl StateAccessor,
    ) -> Result<(), anyhow::Error> {
        let sender_nonce = self
            .nonces
            .get(credential_id, state_checkpoint)?
            .unwrap_or_default();

        anyhow::ensure!(
            sender_nonce == nonce,
            "Tx bad nonce for credential id {}, expected: {}, but found: {}",
            credential_id,
            nonce,
            sender_nonce
        );
        Ok(())
    }

    /// Marks a transaction as attempted, ensuring that future attempts at execution will fail
    pub fn mark_tx_attempted(
        &self,
        credential_id: &CredentialId,
        tx_scratchpad: &mut TxScratchpad<S>,
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
