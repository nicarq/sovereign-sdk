use sov_modules_api::batch::BatchWithId;
use sov_modules_api::hooks::ApplyBatchHooks;
use sov_modules_api::{Context, WorkingSet};
#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_zk_cycle_macros::cycle_tracker;
#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_zk_cycle_utils::print_cycle_count;

use crate::{SequencerOutcome, SequencerRegistry};

impl<C: Context, Da: sov_modules_api::DaSpec> ApplyBatchHooks<Da> for SequencerRegistry<C, Da> {
    type Context = C;
    type BatchResult = SequencerOutcome<Da>;

    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    fn begin_batch_hook(
        &self,
        _batch: &mut BatchWithId,
        sender: &Da::Address,
        working_set: &mut WorkingSet<C>,
    ) -> anyhow::Result<()> {
        #[cfg(all(target_os = "zkvm", feature = "bench"))]
        print_cycle_count();
        if !self.is_sender_allowed(sender, working_set) {
            anyhow::bail!("sender {} is not allowed to submit blobs", sender);
        }
        #[cfg(all(target_os = "zkvm", feature = "bench"))]
        print_cycle_count();
        Ok(())
    }

    fn end_batch_hook(
        &self,
        result: Self::BatchResult,
        working_set: &mut WorkingSet<C>,
    ) -> anyhow::Result<()> {
        match result {
            SequencerOutcome::Completed => (),
            SequencerOutcome::Slashed { sequencer } => {
                self.slash_sequencer(&sequencer, working_set);
            }
        }
        Ok(())
    }
}
