#[cfg(all(target_os = "zkvm", feature = "bench"))]
use risc0_cycle_macros::cycle_tracker;
#[cfg(all(target_os = "zkvm", feature = "bench"))]
use risc0_cycle_utils::print_cycle_count;
use sov_modules_api::hooks::ApplyBatchHooks;
use sov_modules_api::{BatchWithId, Spec, StateCheckpoint};

use crate::{AllowedSequencerError, SequencerOutcome, SequencerRegistry};

impl<S: Spec, Da: sov_modules_api::DaSpec> ApplyBatchHooks<Da> for SequencerRegistry<S, Da> {
    type Spec = S;
    type BatchResult = SequencerOutcome;

    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    fn begin_batch_hook(
        &self,
        _batch: &BatchWithId,
        sender: &Da::Address,
        state: &mut StateCheckpoint<S>,
    ) -> anyhow::Result<()> {
        match self.is_sender_allowed(sender, state) {
            Ok(_) | Err(AllowedSequencerError::NotRegistered) => Ok(()),
            Err(AllowedSequencerError::InsufficientStakeAmount { .. }) => {
                anyhow::bail!(
                    "sender {} is not allowed to submit blobs, they are not sufficiently staked",
                    sender
                )
            }
        }
    }

    fn end_batch_hook(
        &self,
        result: Self::BatchResult,
        sender: &Da::Address,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        match result {
            SequencerOutcome::Rewarded(amount) => {
                self.reward_sequencer(sender, amount, state_checkpoint);
            }
            SequencerOutcome::Slashed => {
                self.slash_sequencer(sender, state_checkpoint);
            }
            SequencerOutcome::Ignored | SequencerOutcome::NotRewardable => {}
        };
    }
}
