use sov_state::Storage;

use crate::transaction::AuthenticatedTransactionData;
#[cfg(feature = "native")]
use crate::AccessoryStateReaderAndWriter;
use crate::{Context, Module, Spec, StateCheckpoint, TxState};

/// Hooks that execute within the `StateTransitionFunction::apply_blob` function for each processed transaction.
///
/// The arguments consist of expected concretely implemented associated types for the hooks. At
/// runtime, compatible implementations are selected and utilized by the system to construct its
/// setup procedures and define post-execution routines.
pub trait TxHooks {
    /// The [`Spec`] of the runtime, which defines the relevant types
    type Spec: Spec;

    /// Runs just before a transaction is dispatched to an appropriate module.
    fn pre_dispatch_tx_hook<T: TxState<Self::Spec>>(
        &self,
        _tx: &AuthenticatedTransactionData<Self::Spec>,
        _state: &mut T,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Runs after the tx is dispatched to an appropriate module.
    /// IF this hook returns error rollup panics
    fn post_dispatch_tx_hook<T: TxState<Self::Spec>>(
        &self,
        _tx: &AuthenticatedTransactionData<Self::Spec>,
        _ctx: &Context<Self::Spec>,
        _state: &mut T,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Autoref blanket implementation of the [`TxHooks`] trait.
/// Any module can override the default behavior by implementing the [`TxHooks`] trait.
impl<T: Module> TxHooks for &T {
    type Spec = T::Spec;

    fn pre_dispatch_tx_hook<S: TxState<Self::Spec>>(
        &self,
        _tx: &AuthenticatedTransactionData<Self::Spec>,
        _state: &mut S,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn post_dispatch_tx_hook<S: TxState<Self::Spec>>(
        &self,
        _tx: &AuthenticatedTransactionData<Self::Spec>,
        _ctx: &Context<Self::Spec>,
        _state: &mut S,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Hooks that execute at the beginning and end of each rollup block.
///
/// A rollup block is created at the discretion of the blob-selector -
/// on "based" rollups, this happens every time the DA layer produces a new block, regardless of the number of batches.
/// On "soft-confirmed" rollups, this happens each time the preferred sequencer successfully lands its next batch on the DA layer.
pub trait BlockHooks {
    /// The runtime spec.
    type Spec: Spec;

    /// Runs at the beginning of each rollup block. See trait description for more details.
    fn begin_rollup_block_hook(
        &self,
        _visible_hash: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        _state: &mut StateCheckpoint<Self::Spec>,
    ) {
    }

    /// Hook that runs at the end of block execution. See trait description for more details.
    fn end_rollup_block_hook(&self, _state: &mut StateCheckpoint<Self::Spec>) {}
}

/// Autoref blanket implementation of the [`BlockHooks`] trait.
/// Any module can override the default behavior by implementing the [`BlockHooks`] trait.
impl<T: Module> BlockHooks for &T {
    type Spec = T::Spec;

    fn begin_rollup_block_hook(
        &self,
        _visible_hash: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        _state: &mut StateCheckpoint<Self::Spec>,
    ) {
    }

    fn end_rollup_block_hook(&self, _state: &mut StateCheckpoint<Self::Spec>) {}
}

/// Trait that defines a hook that runs outside of the main slot processing loop.
#[cfg(feature = "native")]
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

/// Autoref blanket implementation of the [`FinalizeHook`] trait.
/// Any module can override the default behavior by implementing the [`FinalizeHook`] trait.
#[cfg(feature = "native")]
impl<T: Module> FinalizeHook for &T {
    type Spec = T::Spec;

    fn finalize_hook(
        &self,
        _root_hash: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        _state: &mut impl AccessoryStateReaderAndWriter,
    ) {
    }
}
