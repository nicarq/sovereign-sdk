use sov_modules_api::capabilities::UniquenessData;
use sov_modules_api::{CredentialId, InfallibleStateAccessor, Spec, StateAccessor, TxHash};

use crate::Uniqueness;

impl<S: Spec> Uniqueness<S> {
    /// Checks the provided uniqueness number.
    pub fn check_uniqueness(
        &self,
        credential_id: &CredentialId,
        transaction_uniqueness: UniquenessData,
        transaction_hash: TxHash,
        state_checkpoint: &mut impl StateAccessor,
    ) -> anyhow::Result<()> {
        match transaction_uniqueness {
            UniquenessData::Nonce(nonce) => {
                self.check_nonce_uniqueness(credential_id, nonce, state_checkpoint)
            }
            UniquenessData::Generation(generation) => self.check_generation_uniqueness(
                credential_id,
                generation,
                transaction_hash,
                state_checkpoint,
            ),
        }
    }

    /// Marks a transaction as attempted, ensuring that future attempts at execution will fail
    pub fn mark_tx_attempted(
        &self,
        credential_id: &CredentialId,
        transaction_generation: UniquenessData,
        transaction_hash: TxHash,
        tx_scratchpad: &mut impl InfallibleStateAccessor,
    ) {
        match transaction_generation {
            UniquenessData::Nonce(_) => self.mark_nonce_tx_attempted(credential_id, tx_scratchpad),
            UniquenessData::Generation(generation) => self.mark_generational_tx_attempted(
                credential_id,
                generation,
                transaction_hash,
                tx_scratchpad,
            ),
        }
    }
}
