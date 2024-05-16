#![deny(missing_docs)]
//! The rollup capabilities module defines "capabilities" that rollup must
//! provide if they wish to use the standard app template.
//! If you don't want to provide these capabilities,
//! you can bypass the Sovereign module-system completely
//! and write a state transition function from scratch.
//! [See here for docs](https://github.com/Sovereign-Labs/sovereign-sdk/blob/nightly/examples/demo-stf/README.md)
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_rollup_interface::da::DaSpec;
use sov_state::Storage;
use thiserror::Error;

use crate::kernel_state::BootstrapWorkingSet;
use crate::module::Context;
use crate::{Gas, GasMeter, KernelWorkingSet, Spec, StateCheckpoint, TxGasMeter, WorkingSet};

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
    /// Spec type
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
    ) -> anyhow::Result<Vec<(Self::Batch, Da::Address)>>
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
        gas_meter: &TxGasMeter<S::Gas>,
        state_checkpoint: &mut StateCheckpoint<S>,
    );
}

/// Authorizes the sequencer to submit and process batches.
pub trait SequencerAuthorization<S: Spec, Da: DaSpec> {
    /// A type-safe struct that should track the staked amount of the sequencer and the eventual execution penalities.
    type SequencerStakeMeter: GasMeter<S::Gas>;

    /// Checks if the sequencer has staked the minimum bond to attest transactions.
    ///
    /// ## Returns
    /// Returns an error if the sequencer is not registered or does not have enough staked amount.
    /// Returns a [`SequencerAuthorization::SequencerStakeMeter`] if the sequencer is registered and has enough staked amount.
    fn authorize_sequencer(
        &self,
        sequencer: &Da::Address,
        base_fee_per_gas: &<S::Gas as Gas>::Price,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> Result<Self::SequencerStakeMeter, anyhow::Error>;

    /// Partially refunds the sequencer's staked amount.
    /// It should simply increase the remaining funds in the sequencer's staked meter.
    ///
    /// ## Use
    /// This method should be called to diminish the penalty amount of the sequencer when
    /// a transaction has a partial amount of the gas needed for pre-execution checks.
    /// The gas locked in the transaction should be refunded to the sequencer.
    ///
    /// Another use is to refund the sequencer's batch deserialization cost when a transaction is correctly deserialized and executed.
    ///
    /// ## Note
    /// This method should always succeed.
    fn refund_sequencer(
        &self,
        sequencer_stake_meter: &mut Self::SequencerStakeMeter,
        refund_amount: u64,
    );

    /// Penalizes the sequencer without slashing his account.
    /// If the sequencer is penalized, the stake amount of the sequencer is reduced, potentially preventing future transactions from being executed.
    ///
    /// ## Note
    /// This method consumes the [`SequencerAuthorization::SequencerStakeMeter`].
    /// It should only be called once the sequencer cannot be penalized anymore.
    /// The penalty should be accumulated in the [`SequencerAuthorization::SequencerStakeMeter`] during the execution of the transaction.
    fn penalize_sequencer(
        &self,
        sequencer: &Da::Address,
        stake_meter: Self::SequencerStakeMeter,
        state_checkpoint: &mut StateCheckpoint<S>,
    );
}

/// Authorizes transactions to be executed.
pub trait RuntimeAuthorization<S: Spec, Da: DaSpec> {
    /// The transaction that is being authorized.
    type Tx;

    /// Resolves the context for a transaction.
    /// TODO(@preston-evans98): This should be a read-only method `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/384>`
    fn resolve_context(
        &self,
        tx: &Self::Tx,
        sequencer: &Da::Address,
        height: u64,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> Result<Context<S>, anyhow::Error>;

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

/// RawTx represents a serialized rollup transaction received from the DA.
#[derive(Debug, PartialEq, Clone, BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
pub struct RawTx {
    /// Serialized transaction.
    pub data: Vec<u8>,
}

/// Error variants that can be raised as a [`AuthenticationError::FatalError`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum FatalError {
    /// Transaction deserialization failed.
    #[error("Transaction deserialization error: {0}")]
    DeserializationFailed(String),
    /// Signature verification failed.
    #[error("Signature verification error: {0}")]
    SigVerificationFailed(String),
    /// Transaction decoding failed.
    #[error("Transaction decoding error: {0}, tx hash: {1:?}")]
    MessageDecodingFailed(String, [u8; 32]),
    /// A variant to capture any other fatal error.
    #[error("Other fatal error: {0}")]
    Other(String),
}

/// Authentication error type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum AuthenticationError {
    /// The transaction authentication failed in a way that should have been detected by the sequencer before they accepted the transaction. The sequencer is slashed.
    #[error("Transaction authentication raised a fatal error, error: {0}")]
    FatalError(FatalError),
    /// The transaction authentication returned an error, but including it could have been an honest mistake. The sequencer should be charged enough to cover the cost of checking the transaction but not slashed.
    #[error("Transaction authentication was invalid. error: {0}.")]
    Invalid(
        /// The reason for the penalization.       
        String,
    ),
}

/// Authenticates raw transactions. Implementations of this trait should provide a way to interpret the raw bytes of the transaction and authenticate it.
/// Typically, the authentication will require checking the signature of the transaction.
pub trait RuntimeAuthenticator {
    /// Decoded message.
    type Decodable;
    /// Authenticated transaction.
    type Tx;
    /// The gas representation used.
    type Gas: Gas;
    /// A struct that tracks the staked amount of the sequencer and the eventual execution penalities.
    type SequencerStakeMeter: GasMeter<Self::Gas>;
    /// Authenticates raw transaction.
    fn authenticate(
        &self,
        tx: &RawTx,
        sequencer_stake_meter: &mut Self::SequencerStakeMeter,
    ) -> Result<(Self::Tx, Self::Decodable), AuthenticationError>;
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
