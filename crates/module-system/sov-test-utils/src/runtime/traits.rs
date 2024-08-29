//! Provides traits which are useful for wrapping a (possibly incomplete) runtime implementation to create a test runtime
//! with configurable hooks.

use sov_accounts::Accounts;
use sov_bank::{Bank, Payable};
use sov_modules_api::hooks::TxHooks;
use sov_modules_api::{DaSpec, DispatchCall, Genesis, RuntimeEventProcessor, Spec, WorkingSet};
use sov_nonces::Nonces;
use sov_sequencer_registry::SequencerRegistry;

/// A struct which contains at least the bank, the accounts and the sequencer registry modules.
pub trait MinimalRuntime<S: Spec, Da: DaSpec>: Default {
    /// Returns a reference to the sequencer registry module.
    fn sequencer_registry(&self) -> &SequencerRegistry<S, Da>;
    /// Returns a reference to the bank module.
    fn bank(&self) -> &Bank<S>;
    /// Returns a reference to the accounts module.
    fn accounts(&self) -> &Accounts<S>;
    /// Returns a reference to the recipient of the base fees.
    /// This is typically either `AttesterIncentives` optimistic or `ProverIncentives` for provable mode respectively.
    fn base_fee_recipient(&self) -> impl Payable<S>;
    /// Returns a reference to the nonces module.
    fn nonces(&self) -> &Nonces<S>;
}

/// A trait which allows access to the contents of the genesis configuration
/// for a [`MinimalRuntime`] which implements [`Genesis`].
pub trait MinimalGenesis<S: Spec>: Genesis<Spec = S> {
    /// The DA layer spec.
    type Da: DaSpec;
    /// Returns a reference to the sequencer registry config.
    fn sequencer_registry_config(
        config: &Self::Config,
    ) -> &<SequencerRegistry<S, Self::Da> as Genesis>::Config;
    /// Returns a reference to the bank config.
    fn bank_config(config: &Self::Config) -> &<Bank<S> as Genesis>::Config;
    /// Returns a reference to the accounts config.
    fn accounts_config(config: &Self::Config) -> &<Accounts<S> as Genesis>::Config;
}

/// A marker trait which bundles a [`MinimalRuntime`] with additional traits that we require
/// before wrapping a runtime into one that can run hooks.
pub trait StandardRuntime<S: Spec, Da: DaSpec>:
    Clone
    + MinimalRuntime<S, Da>
    + DispatchCall<Spec = S>
    + Genesis<Spec = S>
    + RuntimeEventProcessor
    + MinimalGenesis<S>
    + TxHooks<Spec = S, TxState = WorkingSet<S>>
{
}

impl<S: Spec, Da: DaSpec, T> StandardRuntime<S, Da> for T where
    T: Clone
        + MinimalRuntime<S, Da>
        + DispatchCall<Spec = S>
        + Genesis<Spec = S>
        + RuntimeEventProcessor
        + MinimalGenesis<S>
        + TxHooks<Spec = S, TxState = WorkingSet<S>>
{
}
