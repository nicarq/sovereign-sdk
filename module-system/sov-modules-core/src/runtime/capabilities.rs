#![deny(missing_docs)]
//! The rollup capabilities module defines "capabilities" that rollup must
//! provide if they wish to use the standard app template.
//! If you don't want to provide these capabilities,
//! you can bypass the Sovereign module-system completely
//! and write a state transition function from scratch.
//! [See here for docs](https://github.com/Sovereign-Labs/sovereign-sdk/blob/nightly/examples/demo-stf/README.md)

use sov_rollup_interface::da::DaSpec;

use crate::kernel_state::BootstrapWorkingSet;
use crate::module::Context;
use crate::{Gas, GasMeter, KernelWorkingSet, Spec, StateCheckpoint, Storage, WorkingSet};

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
        working_set: &mut KernelWorkingSet<'_, S>,
    ) -> Result<(), anyhow::Error>;

    /// Return the current slot number
    fn true_slot_number(&self, working_set: &mut BootstrapWorkingSet<'_, S>) -> u64;
    /// Return the slot number at which transactions currently *appear* to be executing.
    fn visible_slot_number(&self, working_set: &mut BootstrapWorkingSet<'_, S>) -> u64;
}

/// Hooks allowing the kernel to get access to the DA layer state
pub trait KernelSlotHooks<S: Spec, Da: DaSpec>: Kernel<S, Da> {
    /// Called at the beginning of a slot. Computes the gas price for the slot
    fn begin_slot_hook(
        &self,
        slot_header: &Da::BlockHeader,
        validity_condition: &Da::ValidityCondition,
        pre_state_root: &<<Self::Spec as Spec>::Storage as Storage>::Root,
        working_set: &mut StateCheckpoint<Self::Spec>,
    ) -> <S::Gas as Gas>::Price;
    /// Called at the end of a slot
    fn end_slot_hook(&self, gas_used: &S::Gas, working_set: &mut StateCheckpoint<Self::Spec>);
}

/// BatchSelector decides which batches to process in a current slot.
pub trait BatchSelector<Da: DaSpec> {
    /// Context type
    type Spec: Spec;

    /// The type of batch returned by the selector
    type Batch;

    /// It takes two arguments.
    /// 1. `current_blobs` - blobs that were received from the network for the current slot.
    /// 2. `working_set` - the working to access storage.
    /// It returns a vector containing a mix of borrowed and owned blobs.
    fn get_batches_for_this_slot<'a, 'k, I>(
        &self,
        current_blobs: I,
        working_set: &mut KernelWorkingSet<'k, Self::Spec>,
    ) -> anyhow::Result<alloc::vec::Vec<(Self::Batch, Da::Address)>>
    where
        I: IntoIterator<Item = &'a mut Da::BlobTransaction>;
}

/// Enforces gas limits and penalties for transactions.
pub trait GasEnforcer<S: Spec, Da: DaSpec> {
    /// The transaction type that the gas enforcer knows how to parse
    type Tx;
    /// Reserves enough gas for the transaction to be processed, if possible.
    #[allow(clippy::result_large_err)]
    fn try_reserve_gas(
        &self,
        tx: &Self::Tx,
        context: &Context<S>,
        gas_price: &<S::Gas as Gas>::Price,
        state_checkpoint: StateCheckpoint<S>,
    ) -> Result<WorkingSet<S>, StateCheckpoint<S>>;

    /// Refunds any remaining gas to the payer after the transaction is processed.
    fn refund_remaining_gas(
        &self,
        tx: &Self::Tx,
        context: &Context<S>,
        gas_meter: &GasMeter<S::Gas>,
        state_checkpoint: &mut StateCheckpoint<S>,
    );
}

/// Deduplicates transactions to prevent double-spending.
pub trait TransactionDeduplicator<S: Spec, Da: DaSpec> {
    /// The transaction type that the deduplicator knows how to parse.
    type Tx;
    /// Prevents duplicate transactions from running.
    fn check_uniqueness(
        &self,
        tx: &Self::Tx,
        context: &Context<S>,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> Result<(), anyhow::Error>;

    /// Marks a transaction as having been executed, preventing it from executing again.
    fn mark_tx_attempted(
        &self,
        tx: &Self::Tx,
        sequencer: &Da::Address,
        state_checkpoint: &mut StateCheckpoint<S>,
    );
}

/// Resolves the context for a transaction.
pub trait ContextResolver<S: Spec, Da: DaSpec> {
    /// The transaction type that the resolver knows how to parse.
    type Tx;
    /// Resolves the context for a transaction.
    // TODO(@preston-evans98): This should be a read-only method https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/384
    fn resolve_context(
        &self,
        tx: &Self::Tx,
        sequencer: &Da::Address,
        height: u64,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> Context<S>;
}

#[cfg(feature = "mocks")]
pub mod mocks {
    //! Mocks for the rollup capabilities module

    use sov_rollup_interface::da::DaSpec;

    use super::{BatchSelector, Kernel, Spec};
    use crate::capabilities::BootstrapWorkingSet;
    use crate::{KernelWorkingSet, StateCheckpoint};

    /// A mock kernel for use in tests
    #[derive(Debug, Clone, derivative::Derivative)]
    #[derivative(Default(bound = ""))]
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
        pub fn genesis_ws(ws: &mut StateCheckpoint<S>) -> KernelWorkingSet<'_, S> {
            let kernel = Self::new(0, 0);
            let ws = KernelWorkingSet::from_kernel(&kernel, ws);
            ws
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
            _working_set: &mut KernelWorkingSet<'_, S>,
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
            _working_set: &mut crate::KernelWorkingSet<'k, Self::Spec>,
        ) -> anyhow::Result<alloc::vec::Vec<(Self::Batch, Da::Address)>>
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
