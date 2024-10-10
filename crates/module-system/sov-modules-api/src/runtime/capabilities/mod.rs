#![deny(missing_docs)]
//! The rollup capabilities module defines "capabilities" that rollup must
//! provide if they wish to use the standard app template.
//! If you don't want to provide these capabilities,
//! you can bypass the Sovereign module-system completely
//! and write a state transition function from scratch.
//! [See here for docs](https://github.com/Sovereign-Labs/sovereign-sdk/blob/nightly/examples/demo-stf/README.md)
pub mod auth;
mod batch_selector;
mod kernel;
mod proof;
pub use auth::*;
pub use batch_selector::*;
pub use kernel::*;
mod gas;
pub use gas::*;
pub use proof::ProofProcessor;
use sov_rollup_interface::da::DaSpec;
mod sequencer;
pub use sequencer::*;

use crate::Spec;

/// Wrapper around an inner type that prevents accessing it.
pub struct Guard<T> {
    inner: T,
}

impl<T> Guard<T> {
    /// Create a new guarded instance.
    pub fn new(inner: T) -> Self {
        Self { inner }
    }
}

/// Indicates that a type provides the necessary capabilities for a runtime.
pub trait HasCapabilities<S: Spec, Da: DaSpec> {
    /// The concrete implementation of the capabilities.
    type Capabilities<'a>: GasEnforcer<S, Da>
        + SequencerAuthorization<S, Da>
        + TransactionAuthorizer<S, Da, AuthorizationData = Self::AuthorizationData>
        + ProofProcessor<S, Da>
        + SequencerRemuneration<S, Da>
    where
        Self: 'a;

    /// The type that is passed to the authorizer.
    type AuthorizationData;

    /// Fetches the capabilities from the runtime.
    ///
    /// This method is only intended to be used internally on the [`HasCapabilities`] trait, if you
    /// need to access a capability do so with the constructor method.
    ///
    /// The returned struct is wrapped in a guard to prevent access from code outside of the trait.
    /// Without the guard it would be possible to implement an override for a capability and
    /// accidently use the default implementation leading subtle type mismatch bugs.
    ///
    /// For example, if I override [`HasCapabilities::gas_enforcer`] to return a different [`GasEnforcer`]
    /// implementation but then used `HasCapabilities::capabilities().try_reserve_gas` instead of
    /// `HasCapabilities::gas_enforcer().try_reserve_gas` I would use the default implementation instead of
    /// the override.
    fn capabilities(&self) -> Guard<Self::Capabilities<'_>>;

    /// Returns the [`GasEnforcer`] implementation on [`HasCapabilities::Capabilities`].
    ///
    /// This method can be overriden to provide a custom implementation.
    fn gas_enforcer(&self) -> impl GasEnforcer<S, Da> {
        self.capabilities().inner
    }

    /// Returns the [`SequencerAuthorization`] implementation on [`HasCapabilities::Capabilities`].
    ///
    /// This method can be overriden to provide a custom implementation.
    fn sequencer_authorization(&self) -> impl SequencerAuthorization<S, Da> {
        self.capabilities().inner
    }

    /// Returns the [`TransactionAuthorizer`] implementation on [`HasCapabilities::Capabilities`].
    ///
    /// This method can be overriden to provide a custom implementation.
    fn transaction_authorizer(
        &self,
    ) -> impl TransactionAuthorizer<S, Da, AuthorizationData = Self::AuthorizationData> {
        self.capabilities().inner
    }

    /// Returns the [`ProofProcessor`] implementation on [`HasCapabilities::Capabilities`].
    ///
    /// This method can be overriden to provide a custom implementation.
    fn proof_processor(&self) -> impl ProofProcessor<S, Da> {
        self.capabilities().inner
    }

    /// Returns the [`SequencerRemuneration`] implementation on [`HasCapabilities::Capabilities`].
    ///
    /// This method can be overriden to provide a custom implementation.
    fn sequencer_remuneration(&self) -> impl SequencerRemuneration<S, Da> {
        self.capabilities().inner
    }
}

#[cfg(feature = "test-utils")]
pub mod mocks {
    //! Mocks for the rollup capabilities module

    #[cfg(feature = "native")]
    use super::KernelWithSlotMapping;
    use super::{Kernel, Spec};
    use crate::BootstrapWorkingSet;

    /// A mock kernel for use in tests
    #[derive(Debug, Clone, Default)]
    pub struct MockKernel<S> {
        /// The current slot number
        pub true_slot_number: u64,
        /// The slot number at which transactions appear to be executing
        pub visible_slot_number: u64,
        phantom: core::marker::PhantomData<S>,
    }

    impl<S: Spec> MockKernel<S> {
        /// Create a new mock kernel with the given slot number
        pub fn new(true_slot_number: u64, visible_height: u64) -> Self {
            Self {
                true_slot_number,
                visible_slot_number: visible_height,
                phantom: core::marker::PhantomData,
            }
        }

        /// Simply increases all the heights by one
        pub fn increase_heights(&mut self) {
            self.true_slot_number += 1;
            self.visible_slot_number += 1;
        }
    }

    #[cfg(feature = "native")]
    impl<S: Spec> KernelWithSlotMapping<S> for MockKernel<S> {
        fn visible_slot_number_at(
            &self,
            true_slot_number: u64,
            _state: &mut crate::ApiStateAccessor<S>,
        ) -> u64 {
            true_slot_number
        }
    }

    impl<S: Spec> Kernel<S> for MockKernel<S> {
        fn true_slot_number(&self, _ws: &mut BootstrapWorkingSet<'_, S::Storage>) -> u64 {
            self.true_slot_number
        }
        fn next_visible_slot_number(&self, _ws: &mut BootstrapWorkingSet<'_, S::Storage>) -> u64 {
            self.visible_slot_number
        }
    }
}
