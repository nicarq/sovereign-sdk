#![deny(missing_docs)]
//! The rollup capabilities module defines "capabilities" that rollup must
//! provide if they wish to use the standard app template.
//! If you don't want to provide these capabilities,
//! you can bypass the Sovereign module-system completely
//! and write a state transition function from scratch.
//! [See here for docs](https://github.com/Sovereign-Labs/sovereign-sdk/blob/nightly/examples/demo-stf/README.md)
use core::fmt;
use std::fmt::Debug;
pub mod auth;
pub use auth::*;
use sov_rollup_interface::da::DaSpec;
use sov_state::Storage;

use crate::module::Context;
use crate::transaction::{AuthenticatedTransactionData, TransactionConsumption};
use crate::{
    BootstrapWorkingSet, Gas, GasMeter, KernelWorkingSet, PreExecWorkingSet, Spec, StateCheckpoint,
    TxScratchpad, WorkingSet,
};

/// Indicates that a type provides the necessary capabilities for a runtime.
pub trait HasCapabilities<S: Spec, Da: DaSpec> {
    /// The concrete implementation of the capabilities.
    type Capabilities<'a>: GasEnforcer<S, Da, PreExecChecksMeter = Self::SequencerStakeMeter>
        + SequencerAuthorization<S, Da, SequencerStakeMeter = Self::SequencerStakeMeter>
        + RuntimeAuthorization<
            S,
            Da,
            SequencerStakeMeter = Self::SequencerStakeMeter,
            AuthorizationData = Self::AuthorizationData,
        >
    where
        Self: 'a;

    /// The type used to meter gas for operations invoked by the sequencer
    /// (e.g. transaction deserialization, failing nonce checks)
    // Note: We require an extra associated type here because `Capabilities` has
    // a lifetime and rustc isn't smart enough to know that he lifetime of `SequencerAuthorization::SequencerStakeMeter`
    // doesn't depend on the lifetime of capabilities.
    type SequencerStakeMeter: GasMeter<S::Gas>;

    /// The type that is passed to the authorizer.
    type AuthorizationData;

    /// Fetches the capabilities from the runtime.
    fn capabilities(&self) -> Self::Capabilities<'_>;
}

/// The kernel is responsible for managing the inputs to the `apply_blob` method.
/// A simple implementation will simply process all blobs in the order that they appear,
/// while a second will support a "preferred sequencer" with some limited power to reorder blobs
/// in order to give out soft confirmations.
pub trait Kernel<S: Spec, Da: DaSpec>: BatchSelector<Da, Spec = S> + Default + Sync + Send {
    /// GenesisConfig type.
    type GenesisConfig: Send + Sync;

    #[cfg(feature = "native")]
    /// GenesisPaths type.
    type GenesisPaths: Send + Sync;

    /// Initialize the kernel at genesis
    fn genesis(
        &self,
        config: &Self::GenesisConfig,
        state: &mut KernelWorkingSet<'_, S>,
    ) -> Result<(), anyhow::Error>;

    /// Return the current slot number
    fn true_slot_number(&self, state: &mut BootstrapWorkingSet<'_, S>) -> u64;
    /// Return the slot number at which transactions currently *appear* to be executing.
    fn visible_slot_number(&self, state: &mut BootstrapWorkingSet<'_, S>) -> u64;
}

/// Hooks allowing the kernel to get access to the DA layer state
pub trait KernelSlotHooks<S: Spec, Da: DaSpec>: Kernel<S, Da> {
    /// Called at the beginning of a slot. Computes the gas price for the slot
    fn begin_slot_hook(
        &self,
        slot_header: &Da::BlockHeader,
        validity_condition: &Da::ValidityCondition,
        pre_state_root: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        state: &mut StateCheckpoint<Self::Spec>,
    ) -> <S::Gas as Gas>::Price;
    /// Called at the end of a slot
    fn end_slot_hook(&self, gas_used: &S::Gas, state: &mut StateCheckpoint<Self::Spec>);
}

/// BatchSelector decides which batches to process in a current slot.
pub trait BatchSelector<Da: DaSpec> {
    /// Spec type
    type Spec: Spec;

    /// The type of batch returned by the selector
    type Batch;

    /// It takes two arguments.
    /// 1. `current_blobs` - blobs that were received from the network for the current slot.
    /// 2. `state` - the working to access storage.
    /// It returns a vector containing a mix of borrowed and owned blobs.
    fn get_batches_for_this_slot<'a, 'k, I>(
        &self,
        current_blobs: I,
        state: &mut KernelWorkingSet<'k, Self::Spec>,
    ) -> anyhow::Result<Vec<(Self::Batch, Da::Address)>>
    where
        I: IntoIterator<Item = &'a mut Da::BlobTransaction>;
}

/// The error type returned by the [`GasEnforcer::try_reserve_gas`] method.
pub struct TryReserveGasError<S: Spec, Meter: GasMeter<S::Gas>> {
    /// The reason why it was not possible to reserve gas.
    pub reason: anyhow::Error,
    /// The pre-execution working set that was used at the time of the error.
    pub pre_exec_working_set: PreExecWorkingSet<S, Meter>,
}

/// Enforces gas limits and penalties for transactions.
pub trait GasEnforcer<S: Spec, Da: DaSpec> {
    /// A gas meter that is used to measure and track the gas used by the pre-execution checks (such as signature checks,
    /// deserialization, and decoding of the transaction).
    type PreExecChecksMeter: GasMeter<S::Gas>;

    /// Checks that the transaction has enough gas to be processed.
    ///
    /// ## Note
    /// This method has to reserve enough gas to cover the pre-execution checks cost of the transaction.
    /// If the transaction doesn't have enough gas to cover the pre-execution checks, the method should return an error.
    ///
    /// ## Behavior
    /// This function **should** charge the transaction sender for the gas locked in the transaction because his balance
    /// may change during the transaction execution.
    #[allow(clippy::result_large_err)]
    fn try_reserve_gas(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        context: &Context<S>,
        pre_exec_working_set: PreExecWorkingSet<S, Self::PreExecChecksMeter>,
    ) -> Result<WorkingSet<S>, TryReserveGasError<S, Self::PreExecChecksMeter>>;

    /// Allocates the gas consumed by the transaction to the base fee and the tip recipients.
    /// This method should not fail.
    ///
    /// ## Correctness note
    /// TODO(@theochap): The rollup developper has to make sure to pre-allocate enough gas to prevent the
    /// transaction sender from underpaying for this operation.
    fn allocate_consumed_gas(
        &self,
        tx_consumption: &TransactionConsumption<S::Gas>,
        tx_scratchpad: &mut TxScratchpad<S>,
    );

    /// Refunds any remaining gas to the payer after the transaction is processed.
    /// This method should not fail.
    ///
    /// ## Correctness note
    /// TODO(@theochap): The rollup developper has to make sure to pre-allocate enough gas to prevent the
    /// transaction sender from underpaying for this operation.
    fn refund_remaining_gas(
        &self,
        context: &Context<S>,
        tx_consumption: &TransactionConsumption<S::Gas>,
        tx_scratchpad: &mut TxScratchpad<S>,
    );
}

/// An error that can be returned within the [`SequencerAuthorization::authorize_sequencer`] capability.
pub struct AuthorizeSequencerError<S: Spec> {
    /// The reason why the sequencer was not authorized.
    pub reason: anyhow::Error,
    /// A [`TxScratchpad`] that contains all the changes made during the transaction processing
    pub tx_scratchpad: TxScratchpad<S>,
}

impl<S: Spec> Debug for AuthorizeSequencerError<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("AuthorizeSequencerError")
            .field(&self.reason)
            .finish()
    }
}

/// Authorizes the sequencer to submit and process batches.
pub trait SequencerAuthorization<S: Spec, Da: DaSpec> {
    /// A type-safe struct that should track the staked amount of the sequencer and the eventual execution penalities.
    type SequencerStakeMeter: GasMeter<S::Gas>;

    /// Checks if the sequencer has staked the minimum bond to attest transactions.
    ///
    /// ## Returns
    /// Returns a [`AuthorizeSequencerError`] error if the sequencer is not registered or does not have enough staked amount.
    /// Returns a [`PreExecWorkingSet`] if the sequencer is registered and has enough staked amount.
    fn authorize_sequencer(
        &self,
        sequencer: &Da::Address,
        base_fee_per_gas: &<S::Gas as Gas>::Price,
        tx_scratchpad: TxScratchpad<S>,
    ) -> Result<PreExecWorkingSet<S, Self::SequencerStakeMeter>, AuthorizeSequencerError<S>>;

    /// Penalizes the sequencer without slashing his account.
    /// If the sequencer is penalized, the stake amount of the sequencer is reduced, potentially preventing future transactions from being executed.
    ///
    /// ## Note
    /// This method consumes the [`PreExecWorkingSet`].
    /// It should only be called once the sequencer cannot be penalized anymore.
    /// The penalty should be accumulated in the [`SequencerAuthorization::SequencerStakeMeter`] during the execution of the transaction.
    fn penalize_sequencer(
        &self,
        sequencer: &Da::Address,
        pre_exec_ws: PreExecWorkingSet<S, Self::SequencerStakeMeter>,
    ) -> TxScratchpad<S>;
}

#[cfg(feature = "mocks")]
pub mod mocks {
    //! Mocks for the rollup capabilities module

    use sov_rollup_interface::da::DaSpec;

    use super::{BatchSelector, Kernel, Spec};
    use crate::capabilities::BootstrapWorkingSet;
    use crate::{KernelWorkingSet, StateCheckpoint};

    /// A mock kernel for use in tests
    #[derive(Debug, Clone, Default)]
    pub struct MockKernel<S, Da> {
        /// The current slot number
        pub true_slot_number: u64,
        /// The slot number at which transactions appear to be executing
        pub visible_slot_number: u64,
        phantom: core::marker::PhantomData<(S, Da)>,
    }

    impl<S: Spec, Da: DaSpec> MockKernel<S, Da> {
        /// Create a new mock kernel with the given slot number
        pub fn new(true_slot_number: u64, visible_height: u64) -> Self {
            Self {
                true_slot_number,
                visible_slot_number: visible_height,
                phantom: core::marker::PhantomData,
            }
        }

        /// The genesis working set
        pub fn genesis_ws(state_checkpoint: &mut StateCheckpoint<S>) -> KernelWorkingSet<'_, S> {
            let kernel = Self::new(0, 0);
            KernelWorkingSet::from_kernel(&kernel, state_checkpoint)
        }
    }

    impl<S: Spec, Da: DaSpec> Kernel<S, Da> for MockKernel<S, Da> {
        fn true_slot_number(&self, _ws: &mut BootstrapWorkingSet<'_, S>) -> u64 {
            self.true_slot_number
        }
        fn visible_slot_number(&self, _ws: &mut BootstrapWorkingSet<'_, S>) -> u64 {
            self.visible_slot_number
        }

        type GenesisConfig = ();

        #[cfg(feature = "native")]
        type GenesisPaths = ();

        fn genesis(
            &self,
            _config: &Self::GenesisConfig,
            _state: &mut KernelWorkingSet<'_, S>,
        ) -> Result<(), anyhow::Error> {
            Ok(())
        }
    }

    impl<S: Spec, Da: DaSpec> BatchSelector<Da> for MockKernel<S, Da> {
        type Spec = S;

        type Batch = Da::BlobTransaction;

        fn get_batches_for_this_slot<'a, 'k, I>(
            &self,
            _current_blobs: I,
            _state: &mut crate::KernelWorkingSet<'k, Self::Spec>,
        ) -> anyhow::Result<Vec<(Self::Batch, Da::Address)>>
        where
            I: IntoIterator<Item = &'a mut Da::BlobTransaction>,
        {
            // Ok(current_blobs
            //     .into_iter()
            //     .map(|blob| {
            //         blob.full_data();
            //         blob.clone()
            //     })
            //     .collect())
            todo!()
        }
    }
}
