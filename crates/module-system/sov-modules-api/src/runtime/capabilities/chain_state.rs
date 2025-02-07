use std::convert::Infallible;

use sov_rollup_interface::common::VisibleSlotNumber;
use sov_rollup_interface::da::DaSpec;
use sov_state::{Kernel, Storage, User};

use super::RollupHeight;
use crate::{Gas, KernelStateAccessor, Spec, StateReader, VersionReader};

/// Capabilities allowing the kernel to update and access the DA layer state.
pub trait ChainState {
    /// The runtime spec.
    type Spec: Spec;

    /// Called at the beginning of a slot. Updates the chain state module
    /// and returns the root hash accessible at the current *visible* slot.
    fn synchronise_chain(
        &self,
        slot_header: &<<Self::Spec as Spec>::Da as DaSpec>::BlockHeader,
        pre_state_root: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        state: &mut KernelStateAccessor<'_, Self::Spec>,
    );

    /// Called at the beginning of a non-empty slot. Updates the rollup height
    fn increment_rollup_height(
        &self,
        state: &mut KernelStateAccessor<'_, Self::Spec>,
        visible_slot_number: VisibleSlotNumber,
        user_state_root: &[u8; 32],
    );

    /// Called at the end of a slot. Updates the chain state module
    /// and finalises the state.
    fn finalise_chain_state(
        &self,
        gas_used: &<Self::Spec as Spec>::Gas,
        state: &mut KernelStateAccessor<'_, Self::Spec>,
    );

    /// Returns the base fee per gas accessible at the current *visible* slot.
    ///
    /// ## Note
    /// This method can return `None` if the base fee per gas for the current slot cannot be determined yet.
    /// This can happen when querying a slot too far ahead in the future.
    fn base_fee_per_gas<
        Reader: VersionReader
            + StateReader<User, Error = Infallible>
            + StateReader<Kernel, Error = Infallible>,
    >(
        &self,
        state: &mut Reader,
    ) -> Option<<<Self::Spec as Spec>::Gas as Gas>::Price>;

    /// Returns the slot gas limit accessible at the current *virtual* slot.
    fn block_gas_limit<
        Reader: VersionReader
            + StateReader<User, Error = Infallible>
            + StateReader<Kernel, Error = Infallible>,
    >(
        &self,
        state: &mut Reader,
    ) -> Option<<Self::Spec as Spec>::Gas>;

    /// Returns the visible root hash accessible at the current *visible* rollup height
    ///
    /// ## Note
    /// This method can return `None` if the visible root hash for the current rollup height cannot be determined yet.
    fn visible_hash_for(
        &self,
        rollup_height: RollupHeight,
        state: &mut KernelStateAccessor<'_, Self::Spec>,
    ) -> Option<<<Self::Spec as Spec>::Storage as Storage>::Root>;
}
