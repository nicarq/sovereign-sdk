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

#[cfg(feature = "native")]
use std::sync::Arc;

pub use auth::*;
pub use batch_selector::*;
pub use kernel::*;

mod gas;
pub use gas::*;
pub use proof::ProofProcessor;
mod sequencer;
pub use sequencer::*;
mod chain_state;
pub use chain_state::*;

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
pub trait HasCapabilities<S: Spec> {
    /// The concrete implementation of the capabilities.
    type Capabilities<'a>: GasEnforcer<S>
        + SequencerAuthorization<S>
        + TransactionAuthorizer<S, AuthorizationData = Self::AuthorizationData>
        + ProofProcessor<S>
        + SequencerRemuneration<S>
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
    fn gas_enforcer(&self) -> impl GasEnforcer<S> {
        self.capabilities().inner
    }

    /// Returns the [`SequencerAuthorization`] implementation on [`HasCapabilities::Capabilities`].
    ///
    /// This method can be overriden to provide a custom implementation.
    fn sequencer_authorization(&self) -> impl SequencerAuthorization<S> {
        self.capabilities().inner
    }

    /// Returns the [`TransactionAuthorizer`] implementation on [`HasCapabilities::Capabilities`].
    ///
    /// This method can be overriden to provide a custom implementation.
    fn transaction_authorizer(
        &self,
    ) -> impl TransactionAuthorizer<S, AuthorizationData = Self::AuthorizationData> {
        self.capabilities().inner
    }

    /// Returns the [`ProofProcessor`] implementation on [`HasCapabilities::Capabilities`].
    ///
    /// This method can be overriden to provide a custom implementation.
    fn proof_processor(&self) -> impl ProofProcessor<S> {
        self.capabilities().inner
    }

    /// Returns the [`SequencerRemuneration`] implementation on [`HasCapabilities::Capabilities`].
    ///
    /// This method can be overriden to provide a custom implementation.
    fn sequencer_remuneration(&self) -> impl SequencerRemuneration<S> {
        self.capabilities().inner
    }
}

/// Indicates that a type provides the necessary kernel capabilities for a runtime.
pub trait HasKernel<S: Spec>: Send + Sync + 'static {
    /// The type of blobs that the kernel can process.
    type BlobType;

    /// The concrete implementation of the kernel.
    type Kernel<'a>: Kernel<S>
        + ChainState<Spec = S>
        + BlobSelector<Spec = S, BlobType = Self::BlobType>
    where
        Self: 'a;

    /// Fetches the kernel modules from the runtime.
    ///
    /// This method is only intended to be used internally on the [`HasKernel`] trait, if you
    /// need to access a kernel capability do so with the constructor method.
    ///
    /// The returned struct is wrapped in a guard to prevent access from code outside of the trait.
    /// Without the guard it would be possible to implement an override for a kernel capability and
    /// accidently use the default implementation leading subtle type mismatch bugs.
    ///
    /// For example, if I override [`HasCapabilities::gas_enforcer`] to return a different [`GasEnforcer`]
    /// implementation but then used `HasCapabilities::capabilities().try_reserve_gas` instead of
    /// `HasCapabilities::gas_enforcer().try_reserve_gas` I would use the default implementation instead of
    /// the override.
    fn inner(&self) -> Guard<Self::Kernel<'_>>;

    /// Returns the [`Kernel`] implementation on [`HasKernel::Kernel`].
    fn kernel(&self) -> impl Kernel<S> {
        self.inner().inner
    }

    /// Returns the [`ChainState`] implementation on [`HasKernel::Kernel`].
    fn chain_state(&self) -> impl ChainState<Spec = S> {
        self.inner().inner
    }

    /// Returns the [`BlobSelector`] implementation on [`HasKernel::Kernel`].
    fn blob_selector(&self) -> impl BlobSelector<Spec = S, BlobType = Self::BlobType> {
        self.inner().inner
    }

    /// Returns the [`KernelWithSlotMapping`] implementation on [`HasKernel::Kernel`].
    #[cfg(feature = "native")]
    fn kernel_with_slot_mapping(&self) -> Arc<dyn KernelWithSlotMapping<S>>;
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
        /// The current rollup height
        pub true_rollup_height: u64,
        /// The rollup height at which transactions appear to be executing
        pub visible_rollup_height: u64,
        phantom: core::marker::PhantomData<S>,
    }

    impl<S: Spec> MockKernel<S> {
        /// Create a new mock kernel with the given rollup height
        pub fn new(true_rollup_height: u64, visible_height: u64) -> Self {
            Self {
                true_rollup_height,
                visible_rollup_height: visible_height,
                phantom: core::marker::PhantomData,
            }
        }

        /// Simply increases all the heights by one
        pub fn increase_heights(&mut self) {
            self.true_rollup_height += 1;
            self.visible_rollup_height += 1;
        }
    }

    #[cfg(feature = "native")]
    impl<S: Spec> KernelWithSlotMapping<S> for MockKernel<S> {
        fn visible_rollup_height_at(
            &self,
            true_rollup_height: u64,
            _state: &mut crate::ApiStateAccessor<S>,
        ) -> u64 {
            true_rollup_height
        }

        fn base_fee_per_gas_at(
            &self,
            _height: u64,
            _state: &mut crate::state::ApiStateAccessor<S>,
        ) -> Option<<<S as Spec>::Gas as crate::Gas>::Price> {
            None
        }
    }

    impl<S: Spec> Kernel<S> for MockKernel<S> {
        fn true_rollup_height(&self, _ws: &mut BootstrapWorkingSet<'_, S::Storage>) -> u64 {
            self.true_rollup_height
        }
        fn next_visible_rollup_height(&self, _ws: &mut BootstrapWorkingSet<'_, S::Storage>) -> u64 {
            self.visible_rollup_height
        }
    }
}
