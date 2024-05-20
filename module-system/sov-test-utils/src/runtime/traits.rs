//! Provides traits which are useful for wrapping a (possibly incomplete) runtime implementation to create a test runtime
//! with configurable hooks.

use sov_attester_incentives::AttesterIncentives;
use sov_bank::Bank;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::hooks::{ApplyBatchHooks, TxHooks};
use sov_modules_api::transaction::AuthenticatedTransactionData;
use sov_modules_api::{
    Context, DaSpec, DispatchCall, Genesis, RuntimeEventProcessor, Spec, StateCheckpoint,
    WorkingSet,
};
use sov_modules_stf_blueprint::BatchSequencerOutcome;
use sov_sequencer_registry::SequencerRegistry;

use super::WorkingSetClosure;

/// A struct which contains at least the bank, sequencer registry, and attester incentives modules.
pub trait MinimalRuntime<S: Spec, Da: DaSpec>: Default {
    fn sequencer_registry(&self) -> &SequencerRegistry<S, Da>;
    fn bank(&self) -> &Bank<S>;
    fn attester_incentives(&self) -> &AttesterIncentives<S, Da>;
}

/// A genesis config which contains at least a sequencer registry config.
pub trait MinimalGenesis<S: Spec>: Genesis {
    type Da: DaSpec;
    fn sequencer_registry(
        config: &Self::Config,
    ) -> &<SequencerRegistry<S, Self::Da> as Genesis>::Config;
}

/// A marker trait which bundles a [`MinimalRuntime`] with additional traits that we require
/// before wrapping a runtime into one that can run hooks.
pub trait StandardRuntime<S: Spec, Da: DaSpec>:
    Clone
    + MinimalRuntime<S, Da>
    + DispatchCall<Spec = S>
    + Genesis<Spec = S>
    + RuntimeEventProcessor
    + MinimalGenesis<S>
    + TxHooks<Spec = S, TxState = WorkingSet<S>>
{
}

impl<S: Spec, Da: DaSpec, T> StandardRuntime<S, Da> for T where
    T: Clone
        + MinimalRuntime<S, Da>
        + DispatchCall<Spec = S>
        + Genesis<Spec = S>
        + RuntimeEventProcessor
        + MinimalGenesis<S>
        + TxHooks<Spec = S, TxState = WorkingSet<S>>
{
}

/// The PostTxHookRegistry trait allows a `Runtime` to inject closures into its post transaction hook.
///
/// Implementers must also implement [`TestRuntimeHookOverrides`] to invoke the closures in their post tx hook.
pub trait PostTxHookRegistry<S: Spec, Da: DaSpec>: TestRuntimeHookOverrides<S, Da> {
    fn add_post_dispatch_tx_hook_actions(&self, closures: Vec<WorkingSetClosure<Self>>);
    fn try_get_next(&self) -> Option<WorkingSetClosure<Self>>;
}

/// Allows the implementer to override the hooks in a wrapped runtime.
pub trait TestRuntimeHookOverrides<S: Spec, Da: DaSpec>: StandardRuntime<S, Da> {
    fn pre_dispatch_tx_hook_override(
        &self,
        _tx: &AuthenticatedTransactionData<S>,
        _working_set: &mut <Self as TxHooks>::TxState,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    fn post_dispatch_tx_hook_override(
        &self,
        _tx: &AuthenticatedTransactionData<S>,
        _ctx: &Context<S>,
        _working_set: &mut <Self as TxHooks>::TxState,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn begin_batch_hook_override(
        &self,
        batch: &mut BatchWithId,
        sender: &Da::Address,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> anyhow::Result<()> {
        self.sequencer_registry()
            .begin_batch_hook(batch, sender, state_checkpoint)
    }

    fn end_batch_hook_override(
        &self,
        result: BatchSequencerOutcome,
        sender: &Da::Address,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        // Since we need to make sure the `StfBlueprint` doesn't depend on the module system, we need to
        // convert the `SequencerOutcome` structures manually.
        let seqencer_outcome = match result {
            BatchSequencerOutcome::Rewarded(amount) => {
                sov_sequencer_registry::SequencerOutcome::Rewarded(amount.into())
            }
            BatchSequencerOutcome::Ignored => sov_sequencer_registry::SequencerOutcome::Ignored,
            BatchSequencerOutcome::Slashed(_reason) => {
                sov_sequencer_registry::SequencerOutcome::Slashed
            }
        };

        <SequencerRegistry<S, Da> as ApplyBatchHooks<Da>>::end_batch_hook(
            self.sequencer_registry(),
            seqencer_outcome,
            sender,
            state_checkpoint,
        );
    }

    fn begin_slot_hook_override(
        &self,
        _pre_state_root: S::VisibleHash,
        _working_set: &mut sov_modules_api::VersionedStateReadWriter<StateCheckpoint<S>>,
    ) {
    }

    fn end_slot_hook_override(&self, _working_set: &mut StateCheckpoint<S>) {}

    fn finalize_hook_override(
        &self,
        _root_hash: S::VisibleHash,
        _accessory_working_set: &mut impl sov_modules_api::prelude::StateReaderAndWriter<
            sov_state::namespaces::Accessory,
        >,
    ) {
    }
}
