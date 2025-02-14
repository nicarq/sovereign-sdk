use sov_modules_api::macros::config_value;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{CredentialId, InfallibleStateAccessor, Spec, StateAccessor, TxHash};

use crate::Uniqueness;
impl<S: Spec> Uniqueness<S> {
    pub(crate) fn check_generation_uniqueness(
        &self,
        credential_id: &CredentialId,
        transaction_generation: u64,
        transaction_hash: TxHash,
        state_checkpoint: &mut impl StateAccessor,
    ) -> anyhow::Result<()> {
        let mut senders_buckets = self
            .generations
            .get(credential_id, state_checkpoint)?
            .unwrap_or_default();

        // The "currently active" generations is the range containing the latest seen generation
        // and the previous PAST_TRANSACTION_GENERATIONS
        let latest_generation = senders_buckets
            .last_key_value()
            .map(|(k, _)| *k)
            .unwrap_or(transaction_generation);

        // If we're below the current generation range, always fail
        // Note about the arithmetic: for a given PAST_TRANSACTION_GENERATIONS, the correct
        // comparison is
        // `transaction_generation <= latest_generation - PAST_TRANSACTION_GENERATIONS`.
        // For example, at generation 10 and the last 5 being valid, the valid generations are 6,
        // 7, 8, 9 and 10; thus we fail the transaction if `transaction_generation <= 5`.
        // However, since we need to use saturating_sub, and we want this to be false when this
        // saturates at 0, we instead have to write
        // `transaction_generation < latest_generation - (PAST_TRANSACTION_GENERATIONS - 1)`.
        if transaction_generation
            < latest_generation.saturating_sub(config_value!("PAST_TRANSACTION_GENERATIONS") - 1)
        {
            anyhow::bail!("Bad generation number for credential id: {credential_id}, latest known generation is: {latest_generation}, provided generation {transaction_generation} is older than cutoff limit");
        }
        // If we're above the range, always pass (we will move up the range and prune older
        // generations in mark_tx_attempted())
        if transaction_generation > latest_generation {
            return Ok(());
        }

        // If we're within the active range, we check the hash against previously stored hashes in
        // the same generation
        if let Some(bucket) = senders_buckets.get(&transaction_generation) {
            if bucket.contains(&transaction_hash) {
                anyhow::bail!("Duplicate transaction for credential_id {credential_id} at generation {transaction_generation}: hash {transaction_hash:} has already been seen");
            }
        };
        // If we reach this point, the transaction is not a duplicate. However, we may still need to reject it to avoid overflowing our capacity for
        // remembering past transactions.

        // If we're above the currently active generation range, then we'll prune some old buckets when we accept this tx. Ignore the buckets that will be pruned
        // in calculating the post-size.
        if transaction_generation > latest_generation {
            // Prune older generations
            let next_lower_bound = transaction_generation
                .saturating_sub(config_value!("PAST_TRANSACTION_GENERATIONS"));
            // IMPORTANT: We don't save our changes to the senders buckets here. We'll do that in mark_generational_tx_attempted() if necessary.
            senders_buckets = senders_buckets.split_off(&next_lower_bound);
        }

        let num_txs_after_increment = senders_buckets
            .values()
            .map(|bucket| bucket.len())
            .sum::<usize>()
            + 1;
        if num_txs_after_increment > config_value!("MAX_STORED_TX_HASHES_PER_CREDENTIAL") {
            // If we overflow, compute the next generation number that the user needs and include it in the error message.
            let first_non_empty_generation = senders_buckets
                .keys()
                .next()
                .expect("Bucket was just checked to be non-empty.");
            let last_generation_before_pruning = first_non_empty_generation
                .saturating_add(config_value!("PAST_TRANSACTION_GENERATIONS"));
            anyhow::bail!("Too many transactions for credential_id {credential_id} at generation {transaction_generation}: hash {transaction_hash:} would cause the bucket to overflow. Increment your generation number to a value greater than {last_generation_before_pruning} and try again.");
        }

        Ok(())
    }

    pub(crate) fn mark_generational_tx_attempted(
        &self,
        credential_id: &CredentialId,
        transaction_generation: u64,
        transaction_hash: TxHash,
        tx_scratchpad: &mut impl InfallibleStateAccessor,
    ) {
        let mut senders_buckets = self
            .generations
            .get(credential_id, tx_scratchpad)
            .unwrap_infallible()
            .unwrap_or_default();

        let latest_generation = senders_buckets
            .last_key_value()
            .map(|(k, _)| *k)
            .unwrap_or(transaction_generation);

        // Defensive check - if mark_tx_attempted() is only called for transactions that passed
        // check_uniqueness(), this will never fail
        // See comment in check_uniqueness w.r.t. arithmetic and explaining the `- 1`
        if transaction_generation
            < latest_generation.saturating_sub(config_value!("PAST_TRANSACTION_GENERATIONS") - 1)
        {
            panic!("Attempted marking transaction as executed despite its generation being older than the generation cutoff point");
        }

        // If we're above the currently active generation range, then move the range up, pruning
        // older generations
        if transaction_generation > latest_generation {
            // Prune older generations
            let new_lower_bound = transaction_generation
                .saturating_sub(config_value!("PAST_TRANSACTION_GENERATIONS"));
            senders_buckets = senders_buckets.split_off(&new_lower_bound);
        }

        // Record known transaction hash for this generation
        // Defensively assert it's not a duplicate, again if check_uniqueness() passed this should
        // never fail
        assert!(senders_buckets
            .entry(transaction_generation)
            .or_default()
            .insert(transaction_hash));

        self.generations
            .set(credential_id, &senders_buckets, tx_scratchpad)
            .unwrap_infallible();
    }
}
