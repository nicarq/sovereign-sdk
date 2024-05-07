use sov_bank::Amount;
use sov_modules_api::{Spec, StateAccessor, StateCheckpoint};

use crate::{AllowedSequencer, AllowedSequencerError, SequencerRegistry};

impl<S: Spec, Da: sov_modules_api::DaSpec> SequencerRegistry<S, Da> {
    /// Checks whether `sender` is a registered sequencer with enough staked amount.
    /// If so, `Ok(())`. Otherwise, returns a [`AllowedSequencerError`].
    pub fn authorize_sequencer(
        &self,
        sender: &Da::Address,
        working_set: &mut impl StateAccessor,
    ) -> Result<(), AllowedSequencerError> {
        self.is_sender_allowed(sender, working_set)?;

        Ok(())
    }

    /// Penalizes the sequencer with the `amount` of gas tokens.
    /// This method simply deducts the reward from the sequencer's staked amount.
    ///
    /// # Safety note:
    /// - If the sender is not registered this method silently exits.
    /// - If the sender is registered and the penalty is greater than the sequencer's staked amount, the sequencer's staked amount is set to 0
    /// but the sequencer is not removed from the list of allowed sequencers.
    ///
    /// # Note `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/454>`
    /// This method should be modified to ensure that the penalty is not greater than the sequencer's staked amount.
    pub fn penalize_sequencer(
        &self,
        sender: &Da::Address,
        amount: Amount,
        working_set: &mut StateCheckpoint<S>,
    ) {
        if let Some(AllowedSequencer { address, balance }) =
            self.allowed_sequencers.get(sender, working_set)
        {
            let new_balance = balance.saturating_sub(amount);
            self.allowed_sequencers.set(
                sender,
                &AllowedSequencer {
                    address,
                    balance: new_balance,
                },
                working_set,
            );
        }
    }
}
