use sov_rollup_interface::common::{SlotNumber, VisibleSlotNumber};

use crate::{BootstrapWorkingSet, KernelStateAccessor, Spec, StateCheckpoint};

/// Allows the kernel to map between a rollup height and the visible height at that slot.
/// This is used to enable access to the correct (visible) kernel state during archival queries.
#[cfg(feature = "native")]
pub trait KernelWithSlotMapping<S: Spec>: Sync + Send + 'static {
    /// Gets the visible rollup height as of the given true rollup height.
    // This method takes `ApiStateAccessor` rather than an `impl Trait` because
    // we need it to be object safe
    fn visible_rollup_height_at(
        &self,
        true_rollup_height: SlotNumber,
        state: &mut crate::state::ApiStateAccessor<S>,
    ) -> Option<VisibleSlotNumber>;

    /// Opposite operation of [`KernelWithSlotMapping::visible_rollup_height_at`].
    ///
    /// It's important to keep in mind that there's **no** single true
    /// [`SlotNumber`] for any given [`VisibleSlotNumber`]. This may be true for
    /// some visible slot numbers, but not all of them.
    ///
    /// To obviate this problem, implementors of this trait MUST return the very
    /// **first** true slot number that's associated with the given visible slot
    /// number.
    fn first_true_slot_number_for(
        &self,
        visible_rollup_height: VisibleSlotNumber,
        state: &mut crate::state::ApiStateAccessor<S>,
    ) -> Option<SlotNumber>;

    /// Returns the base fee per gas accessible at the specified rollup height for this state accessor.
    ///
    /// ## Note
    /// This method may return `None` if it is not possible to retrieve the correct base fee per gas from the state.
    fn base_fee_per_gas_at(
        &self,
        height: VisibleSlotNumber,
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

    /// Return the current rollup height
    fn true_rollup_height(&self, state: &mut BootstrapWorkingSet<'_, S::Storage>) -> SlotNumber;
    /// Return the next value of the rollup height at which transactions currently *appear* to be executing.
    fn next_visible_rollup_height(
        &self,
        state: &mut BootstrapWorkingSet<'_, S::Storage>,
    ) -> VisibleSlotNumber;
}
