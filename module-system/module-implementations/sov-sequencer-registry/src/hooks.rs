#[cfg(all(target_os = "zkvm", feature = "bench"))]
use risc0_cycle_macros::cycle_tracker;
#[cfg(all(target_os = "zkvm", feature = "bench"))]
use risc0_cycle_utils::print_cycle_count;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::hooks::ApplyBatchHooks;
use sov_modules_api::{Spec, StateCheckpoint};

use crate::{SequencerOutcome, SequencerRegistry};

impl<S: Spec, Da: sov_modules_api::DaSpec> ApplyBatchHooks<Da> for SequencerRegistry<S, Da> {
    type Spec = S;
    type BatchResult = SequencerOutcome<Da>;

    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    fn begin_batch_hook(
        &self,
        _batch: &mut BatchWithId,
        sender: &Da::Address,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> anyhow::Result<()> {
        #[cfg(all(target_os = "zkvm", feature = "bench"))]
        print_cycle_count();
        if !self.is_sender_allowed(sender, state_checkpoint) {
            anyhow::bail!("sender {} is not allowed to submit blobs", sender);
        }
        #[cfg(all(target_os = "zkvm", feature = "bench"))]
        print_cycle_count();
        Ok(())
    }

    fn end_batch_hook(&self, result: Self::BatchResult, state_checkpoint: &mut StateCheckpoint<S>) {
        match result {
            SequencerOutcome::Rewarded { .. } | SequencerOutcome::Penalized { .. } => {
                // TODO(@vlopes11) Process the actual reward/penalty
            }
            SequencerOutcome::Slashed { sequencer } => {
                self.slash_sequencer(&sequencer, state_checkpoint);
            }
        };
    }
}
