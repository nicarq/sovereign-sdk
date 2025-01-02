use sov_rollup_interface::da::DaSpec;
use sov_state::Storage;

use crate::transaction::AuthenticatedTransactionData;
#[cfg(feature = "native")]
use crate::AccessoryStateReaderAndWriter;
use crate::{Context, KernelStateAccessor, Module, Spec, StateCheckpoint, TxState};

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

/// Autoref blanket implementation of the [`SlotHooks`] trait.
/// Any module can override the default behavior by implementing the [`SlotHooks`] trait.
impl<T: Module> SlotHooks for &T {
    type Spec = T::Spec;

    fn begin_slot_hook(
        &self,
        _visible_hash: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        _state: &mut StateCheckpoint<<Self::Spec as Spec>::Storage>,
    ) {
    }

    fn end_slot_hook(&self, _state: &mut StateCheckpoint<<Self::Spec as Spec>::Storage>) {}
}

/// Hooks allowing the runtime to get access to the DA layer state
pub trait KernelSlotHooks {
    /// The runtime spec.
    type Spec: Spec;

    /// Called at the beginning of a slot.
    fn kernel_begin_slot_hook(
        &self,
        _slot_header: &<<Self::Spec as Spec>::Da as DaSpec>::BlockHeader,
        _validity_condition: &<<Self::Spec as Spec>::Da as DaSpec>::ValidityCondition,
        _pre_state_root: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        _state: &mut KernelStateAccessor<'_, <Self::Spec as Spec>::Storage>,
    ) {
    }

    /// Called at the end of a slot
    fn kernel_end_slot_hook(
        &self,
        _gas_used: &<Self::Spec as Spec>::Gas,
        _state: &mut KernelStateAccessor<'_, <Self::Spec as Spec>::Storage>,
    ) {
    }
}

/// Autoref blanket implementation of the [`KernelSlotHooks`] trait.
/// Any module can override the default behavior by implementing the [`KernelSlotHooks`] trait.
impl<T: Module> KernelSlotHooks for &T {
    type Spec = T::Spec;

    fn kernel_begin_slot_hook(
        &self,
        _slot_header: &<<Self::Spec as Spec>::Da as DaSpec>::BlockHeader,
        _validity_condition: &<<Self::Spec as Spec>::Da as DaSpec>::ValidityCondition,
        _pre_state_root: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        _state: &mut KernelStateAccessor<'_, <Self::Spec as Spec>::Storage>,
    ) {
    }

    fn kernel_end_slot_hook(
        &self,
        _gas_used: &<Self::Spec as Spec>::Gas,
        _state: &mut KernelStateAccessor<'_, <Self::Spec as Spec>::Storage>,
    ) {
    }
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
