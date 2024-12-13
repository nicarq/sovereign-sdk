mod accessors;
mod events;
mod traits;

#[cfg(test)]
mod tests;

#[cfg(any(feature = "test-utils", feature = "evm"))]
pub use accessors::UnmeteredStateWrapper;
pub use accessors::{
    AccessoryDelta, BootstrapWorkingSet, GenesisStateAccessor, KernelStateAccessor,
    PreExecWorkingSet, StateCheckpoint, StateProvider, TxChangeSet, TxScratchpad, WorkingSet,
};
#[cfg(feature = "native")]
pub use accessors::{AccessoryStateCheckpoint, ApiStateAccessor};
pub use events::TypedEvent;
#[cfg(feature = "native")]
use sov_rollup_interface::ProvableHeightTracker;
#[cfg(feature = "native")]
pub use traits::ProvenStateAccessor;
pub use traits::{
    AccessoryStateReader, AccessoryStateReaderAndWriter, AccessoryStateWriter, GenesisState,
    InfallibleKernelStateAccessor, InfallibleStateAccessor, InfallibleStateReaderAndWriter,
    KernelWriter, ProvableStateReader, ProvableStateWriter, StateAccessor, StateAccessorError,
    StateReader, StateReaderAndWriter, StateWriter, TxState, VersionReader,
};

#[cfg(feature = "native")]
/// Utilities to allow tracking the maximum provable height of the rollup.
pub mod provable_height_tracker {
    use super::*;
    use crate::capabilities::HasKernel;
    use crate::rest::StateUpdateReceiver;
    use crate::Spec;
    /// A default implementation of [`ProvableHeightTracker`].
    /// Tracks the maximum height provable in the rollup by using the kernel of the rollup.
    pub struct MaximumProvableHeight<S: Spec, K: HasKernel<S>> {
        state_update_receiver: StateUpdateReceiver<S::Storage>,
        kernel: K,
    }

    impl<S: Spec, K: HasKernel<S>> MaximumProvableHeight<S, K> {
        /// Creates a new [`MaximumProvableHeight`].
        pub fn new(state_update_receiver: StateUpdateReceiver<S::Storage>, kernel: K) -> Self {
            Self {
                state_update_receiver,
                kernel,
            }
        }
    }

    impl<S: Spec, K: HasKernel<S>> ProvableHeightTracker for MaximumProvableHeight<S, K> {
        fn maximum_provable_height(&self) -> u64 {
            let storage = self.state_update_receiver.borrow().storage.clone();
            let checkpoint = StateCheckpoint::new(storage, &self.kernel.kernel());
            // Substract 1 because the state root at slot height `i` is only available at slot height `i + 1`.
            checkpoint.rollup_height_to_access().saturating_sub(1)
        }
    }

    /// An implementation of [`ProvableHeightTracker`] that can be used to specify an infinite height.
    #[cfg(feature = "test-utils")]
    #[derive(Clone, Debug, Default)]
    pub struct InfiniteHeight;

    #[cfg(feature = "test-utils")]
    impl InfiniteHeight {}

    #[cfg(feature = "test-utils")]
    impl ProvableHeightTracker for InfiniteHeight {
        fn maximum_provable_height(&self) -> u64 {
            u64::MAX
        }
    }
}
