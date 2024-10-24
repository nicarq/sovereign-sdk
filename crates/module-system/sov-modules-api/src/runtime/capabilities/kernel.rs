use crate::{BootstrapWorkingSet, KernelStateAccessor, Spec, StateCheckpoint};

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

    /// Returns the base fee per gas accessible at the specified true slot number for this state accessor.
    ///
    /// ## Usage
    /// This method should first map the true slot number to the visible slot number using [`KernelWithSlotMapping::visible_slot_number_at`]
    /// and then retrieve the base fee per gas from the state at the visible slot number.
    ///
    /// ## Note
    /// This method may return `None` if it is not possible to retrieve the correct base fee per gas from the state.
    fn base_fee_per_gas_at(
        &self,
        height: u64,
        state: &mut crate::state::ApiStateAccessor<S>,
    ) -> Option<<<S as Spec>::Gas as crate::Gas>::Price>;
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
