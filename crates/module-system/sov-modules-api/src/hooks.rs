use sov_rollup_interface::da::DaSpec;
use sov_state::Storage;

use crate::transaction::AuthenticatedTransactionData;
use crate::{
    AccessoryStateReaderAndWriter, Context, KernelStateAccessor, Spec, StateCheckpoint, TxState,
};

/// Hooks that execute within the `StateTransitionFunction::apply_blob` function for each processed transaction.
///
/// The arguments consist of expected concretely implemented associated types for the hooks. At
/// runtime, compatible implementations are selected and utilized by the system to construct its
/// setup procedures and define post-execution routines.
pub trait TxHooks {
    /// The [`Spec`] of the runtime, which defines the relevant types
    type Spec: Spec;

    /// The state accessor expected by the methods of this hook.
    ///
    /// We use an associated type rather than `impl Trait` since this encodes that the
    /// state accessor is always the same type rather than being arbitrarily chosen by the caller at each invocation.
    /// This in turn allows `Self::TxState` to be used as an argument in a closure, which is needed for testing.
    ///
    /// In the short term, this causes some undesirable leakage of the concrete `WorkingSet` type
    /// into these hook methods. This will be fixed when Rust supports `impl Trait` as an associated type
    /// <https://github.com/rust-lang/rust/pull/120700>.
    type TxState: TxState<Self::Spec> + Send + 'static;

    /// Runs just before a transaction is dispatched to an appropriate module.
    fn pre_dispatch_tx_hook(
        &self,
        _tx: &AuthenticatedTransactionData<Self::Spec>,
        _state: &mut Self::TxState,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Runs after the tx is dispatched to an appropriate module.
    /// IF this hook returns error rollup panics
    fn post_dispatch_tx_hook(
        &self,
        _tx: &AuthenticatedTransactionData<Self::Spec>,
        _ctx: &Context<Self::Spec>,
        _state: &mut Self::TxState,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Hooks related to the Sequencer functionality.
/// In essence, the sequencer locks a bond at the beginning of the `StateTransitionFunction::apply_blob`,
/// and is rewarded once a blob of transactions is processed.
pub trait ApplyBatchHooks {
    /// The runtime spec.
    type Spec: Spec;
    /// The result of applying a batch.
    type BatchResult;

    /// Runs at the beginning of apply_blob, locks the sequencer bond.
    /// If this hook returns Err, batch is not applied
    fn begin_batch_hook(
        &self,
        _sender: &<<Self::Spec as Spec>::Da as DaSpec>::Address,
        _state_checkpoint: &mut StateCheckpoint<<Self::Spec as Spec>::Storage>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Executes at the end of apply_blob and rewards or slashed the sequencer
    /// If this hook returns Err rollup panics
    fn end_batch_hook(
        &self,
        _result: &Self::BatchResult,
        _state_checkpoint: &mut StateCheckpoint<<Self::Spec as Spec>::Storage>,
    ) {
    }
}

/// Type alias that contains the height of a given transition
pub type TransitionHeight = u64;

/// Hooks that execute during the `StateTransitionFunction::begin_slot` and `end_slot` functions.
pub trait SlotHooks {
    /// The runtime spec.
    type Spec: Spec;

    /// Hook that runs at the beginning of the `apply_slot` function inside the `StateTransitionFunction`.
    fn begin_slot_hook(
        &self,
        _visible_hash: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        _state: &mut StateCheckpoint<<Self::Spec as Spec>::Storage>,
    ) {
    }

    /// Hook that runs at the end of the `apply_slot` function inside the `StateTransitionFunction`.
    fn end_slot_hook(&self, _state: &mut StateCheckpoint<<Self::Spec as Spec>::Storage>) {}
}

/// Hooks allowing the runtime to get access to the DA layer state
pub trait KernelSlotHooks {
    /// The runtime spec.
    type Spec: Spec;

    /// Called at the beginning of a slot.
    fn begin_slot_hook(
        &self,
        _slot_header: &<<Self::Spec as Spec>::Da as DaSpec>::BlockHeader,
        _validity_condition: &<<Self::Spec as Spec>::Da as DaSpec>::ValidityCondition,
        _pre_state_root: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        _state: &mut KernelStateAccessor<'_, <Self::Spec as Spec>::Storage>,
    ) {
    }

    /// Called at the end of a slot
    fn end_slot_hook(
        &self,
        _gas_used: &<Self::Spec as Spec>::Gas,
        _state: &mut KernelStateAccessor<'_, <Self::Spec as Spec>::Storage>,
    ) {
    }
}

/// Trait that defines a hook that runs outside of the main slot processing loop.
pub trait FinalizeHook {
    /// The runtime spec.
    type Spec: Spec;

    /// Hook that defines logic that runs after calculating the new state root hash.
    /// At this point, it is impossible to alter state variables because the state root is fixed.
    /// However, non-state data can be modified.
    /// Use this hook to perform any post-processing changes to the accessory state (changes to the accessory
    /// state are not proved and hence don't affect the state root hash).
    fn finalize_hook(
        &self,
        _root_hash: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        _state: &mut impl AccessoryStateReaderAndWriter,
    ) {
    }
}
