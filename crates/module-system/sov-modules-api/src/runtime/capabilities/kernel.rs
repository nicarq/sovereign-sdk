use sov_rollup_interface::da::DaSpec;
use sov_state::Storage;

use crate::{BootstrapWorkingSet, Gas, KernelStateAccessor, Spec, StateCheckpoint};

/// Allows the kernel to map between a slot number and the visible height at that slot.
/// This is used to enable access to the correct (visible) kernel state during archival queries.
#[cfg(feature = "native")]
pub trait KernelWithSlotMapping<S: Spec>: Sync + Send + 'static {
    /// Gets the visible slot number as of the given true slot number.
    // This method takes `ApiStateAccessor` rather than an `impl Trait` because
    // we need it to be object safe
    fn visible_slot_number_at(
        &self,
        true_slot_number: u64,
        state: &mut crate::state::ApiStateAccessor<S>,
    ) -> u64;
}

/// The kernel is responsible for managing the inputs to the `apply_blob` method.
/// A simple implementation will simply process all blobs in the order that they appear,
/// while a second will support a "preferred sequencer" with some limited power to reorder blobs
/// in order to give out soft confirmations.
pub trait Kernel<S: Spec> {
    /// Returns a [`KernelStateAccessor`] for the given [`StateCheckpoint`].
    fn accessor<'a>(
        &self,
        state: &'a mut StateCheckpoint<S::Storage>,
    ) -> KernelStateAccessor<'a, S::Storage> {
        KernelStateAccessor::from_checkpoint(self, state)
    }

    /// Return the current slot number
    fn true_slot_number(&self, state: &mut BootstrapWorkingSet<'_, S::Storage>) -> u64;
    /// Return the next value of the slot number at which transactions currently *appear* to be executing.
    fn next_visible_slot_number(&self, state: &mut BootstrapWorkingSet<'_, S::Storage>) -> u64;
}

/// Hooks allowing the kernel to get access to the DA layer state
pub trait KernelSlotHooks<S: Spec> {
    /// Called at the beginning of a slot. Computes the gas price for the slot
    /// Returns the visible root hash accessible at the current *virtual* rollup height
    fn begin_slot_hook(
        &self,
        slot_header: &<S::Da as DaSpec>::BlockHeader,
        validity_condition: &<S::Da as DaSpec>::ValidityCondition,
        pre_state_root: &<S::Storage as Storage>::Root,
        state: &mut KernelStateAccessor<'_, S::Storage>,
    ) -> <S::Storage as Storage>::Root;

    /// Called at the end of a slot
    fn end_slot_hook(&self, gas_used: &S::Gas, state: &mut KernelStateAccessor<'_, S::Storage>);

    /// Returns the base fee per gas accessible at the current *virtual* slot.
    fn base_fee_per_gas(&self, state: &mut StateCheckpoint<S::Storage>) -> <S::Gas as Gas>::Price;
}
