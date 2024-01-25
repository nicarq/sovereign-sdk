use sov_modules_core::{
    AccessoryWorkingSet, Context, Spec, Storage, VersionedWorkingSet, WorkingSet,
};
use sov_rollup_interface::da::DaSpec;

use crate::batch::BatchWithId;
use crate::transaction::Transaction;

/// Hooks that execute within the `StateTransitionFunction::apply_blob` function for each processed transaction.
///
/// The arguments consist of expected concretely implemented associated types for the hooks. At
/// runtime, compatible implementations are selected and utilized by the system to construct its
/// setup procedures and define post-execution routines.
pub trait TxHooks {
    type Context: Context;
    type PreArg;
    type PreResult;

    /// Runs just before a transaction is dispatched to an appropriate module.
    fn pre_dispatch_tx_hook(
        &self,
        tx: &Transaction<Self::Context>,
        working_set: &mut WorkingSet<Self::Context>,
        arg: &Self::PreArg,
    ) -> anyhow::Result<Self::PreResult>;

    /// Runs after the tx is dispatched to an appropriate module.
    /// IF this hook returns error rollup panics
    fn post_dispatch_tx_hook(
        &self,
        tx: &Transaction<Self::Context>,
        ctx: &Self::Context,
        working_set: &mut WorkingSet<Self::Context>,
    ) -> anyhow::Result<()>;
}

/// Hooks related to the Sequencer functionality.
/// In essence, the sequencer locks a bond at the beginning of the `StateTransitionFunction::apply_blob`,
/// and is rewarded once a blob of transactions is processed.
pub trait ApplyBatchHooks<Da: DaSpec> {
    type Context: Context;
    type BatchResult;

    /// Runs at the beginning of apply_blob, locks the sequencer bond.
    /// If this hook returns Err, batch is not applied
    fn begin_batch_hook(
        &self,
        batch: &mut BatchWithId,
        sender: &Da::Address,
        working_set: &mut WorkingSet<Self::Context>,
    ) -> anyhow::Result<()>;

    /// Executes at the end of apply_blob and rewards or slashed the sequencer
    /// If this hook returns Err rollup panics
    fn end_batch_hook(
        &self,
        result: Self::BatchResult,
        working_set: &mut WorkingSet<Self::Context>,
    ) -> anyhow::Result<()>;
}

/// Type alias that contains the height of a given transition
pub type TransitionHeight = u64;

/// Hooks that execute during the `StateTransitionFunction::begin_slot` and `end_slot` functions.
pub trait SlotHooks {
    type Context: Context;

    fn begin_slot_hook(
        &self,
        pre_state_root: &<<Self::Context as Spec>::Storage as Storage>::Root,
        working_set: &mut VersionedWorkingSet<Self::Context>,
    );

    fn end_slot_hook(&self, working_set: &mut WorkingSet<Self::Context>);
}

pub trait FinalizeHook {
    type Context: Context;

    fn finalize_hook(
        &self,
        root_hash: &<<Self::Context as Spec>::Storage as Storage>::Root,
        accessory_working_set: &mut AccessoryWorkingSet<Self::Context>,
    );
}
