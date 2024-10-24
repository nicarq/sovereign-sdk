use std::convert::Infallible;

use sov_rollup_interface::da::DaSpec;
use sov_state::Storage;

use crate::{Gas, KernelStateAccessor, Spec, VersionReader};

/// Capabilities allowing the kernel to update and access the DA layer state.
pub trait ChainState {
    /// The runtime spec.
    type Spec: Spec;

    /// Called at the beginning of a slot. Updates the chain state module
    /// and returns the root hash accessible at the current *virtual* slot.
    fn synchronise_chain(
        &self,
        slot_header: &<<Self::Spec as Spec>::Da as DaSpec>::BlockHeader,
        validity_condition: &<<Self::Spec as Spec>::Da as DaSpec>::ValidityCondition,
        pre_state_root: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        state: &mut KernelStateAccessor<'_, <Self::Spec as Spec>::Storage>,
    );

    /// Called at the end of a slot. Updates the chain state module
    /// and finalises the state.
    fn finalise_chain_state(
        &self,
        gas_used: &<Self::Spec as Spec>::Gas,
        state: &mut KernelStateAccessor<'_, <Self::Spec as Spec>::Storage>,
    );

    /// Returns the base fee per gas accessible at the current *virtual* slot.
    ///
    /// ## Note
    /// This method can return `None` if the base fee per gas for the current slot cannot be determined yet.
    /// This can happen when querying a slot too far ahead in the future.
    fn base_fee_per_gas(
        &self,
        state: &mut impl VersionReader<Error = Infallible>,
    ) -> Option<<<Self::Spec as Spec>::Gas as Gas>::Price>;

    /// Returns the visible root hash accessible at the current *virtual* rollup height
    fn current_visible_hash(
        &self,
        pre_state_root: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        state: &mut KernelStateAccessor<'_, <Self::Spec as Spec>::Storage>,
    ) -> <<Self::Spec as Spec>::Storage as Storage>::Root;
}
