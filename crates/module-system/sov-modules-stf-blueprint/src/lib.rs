#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
mod stf_blueprint;

use sequencer_mode::{registered, unregistered};
use serde::{Deserialize, Serialize};
use sov_modules_api::{
    BatchSequencerReceipt, IncrementalBatch, IterableBatchWithId, VersionReader,
};
mod proof_processing;
use sov_rollup_interface::stf::ProofReceipt;
mod sequencer_mode;
#[cfg(feature = "test-utils")]
mod utils;
/// We export the `apply_tx` function to use inside the simulation endpoints.
pub use sequencer_mode::apply_tx;
pub use sequencer_mode::common::{
    get_gas_used, AuthTxOutput, BatchReceipt, TransactionReceipt, ValidatedAuthOutput,
};
pub use sequencer_mode::registered::{process_tx, PreExecError};
#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_cycle_utils::macros::cycle_tracker;
use sov_modules_api::capabilities::{
    BlobOrigin, BlobSelector, BlobSelectorOutput, ChainState, HasKernel, Kernel,
    TransactionAuthenticator,
};
use sov_modules_api::hooks::{KernelSlotHooks, SlotHooks};
use sov_modules_api::transaction::TransactionConsumption;
pub use sov_modules_api::{BatchWithId, BlobData, Runtime};
use sov_modules_api::{
    BlobDataWithId, DaSpec, Error, ExecutionContext, Gas, GasArray, Genesis, Spec, StateCheckpoint,
};
#[cfg(feature = "native")]
use sov_rollup_interface::da::BlockHeaderTrait;
use sov_rollup_interface::da::RelevantBlobIters;
use sov_rollup_interface::stf::{ApplySlotOutput, StateTransitionFunction};
#[cfg(feature = "native")]
use sov_state::storage::StateUpdate;
use sov_state::{Storage, StorageProof};
pub use stf_blueprint::StfBlueprint;
use thiserror::Error;
use tracing::info;

use crate::unregistered::BatchWithSingleTx;

/// The contents of the receipt for a skipped transaction
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SkippedTxContents<S: Spec> {
    /// The gas consumed by the transaction.
    pub gas_used: S::Gas,
    /// Reason why the transaction was skipped.
    pub error: TxProcessingError,
}

impl<S: Spec> PartialEq for SkippedTxContents<S> {
    fn eq(&self, other: &Self) -> bool {
        self.gas_used == other.gas_used && self.error == other.error
    }
}
impl<S: Spec> Eq for SkippedTxContents<S> {}

/// The transaction processing error.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum TxProcessingError {
    /// Transaction authentication failed.
    #[error(" Transaction authentication failed {0}.")]
    AuthenticationFailed(String),
    /// The transaction had an invalid nonce.
    #[error("The transaction had an invalid nonce, reason: {0}.")]
    IncorrectNonce(String),
    /// Impossible to reserve gas for the transaction to be executed.
    #[error("Impossible to reserve gas for the transaction to be executed, reason: {0}.")]
    CannotReserveGas(String),
    /// Impossible to resolve the context of the transaction.
    #[error("Impossible to resolve the context of the transaction, reason: {0}.")]
    CannotResolveContext(String),
    /// Rejected by a pre-flight check.
    #[error("The transaction was rejected by a pre-flight check.")]
    RejectedByPreFlight,
    /// The transaction ran out of gas
    #[error("The transaction ran out of gas, reason: {0}.")]
    OutOfGas(String),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Error)]
/// The contents of the receipt for a reverted transaction
pub struct RevertedTxContents<S: Spec> {
    /// The gas consumed by the transaction
    pub gas_used: S::Gas,
    /// The reason the tx reverted.
    pub reason: Error,
}

impl<S: Spec> PartialEq for RevertedTxContents<S> {
    fn eq(&self, other: &Self) -> bool {
        self.gas_used == other.gas_used && self.reason == other.reason
    }
}
impl<S: Spec> Eq for RevertedTxContents<S> {}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Error)]
/// The contents of the receipt for a successful transaction
pub struct SuccessfulTxContents<S: Spec> {
    /// The gas consumed by the transaction
    pub gas_used: S::Gas,
}

impl<S: Spec> PartialEq for SuccessfulTxContents<S> {
    fn eq(&self, other: &Self) -> bool {
        self.gas_used == other.gas_used
    }
}
impl<S: Spec> Eq for SuccessfulTxContents<S> {}

/// The effect of a transaction using the STF blueprint.
pub type TxEffect<S> = sov_rollup_interface::stf::TxEffect<TxReceiptContents<S>>;
/// The effect of a batch using the STF blueprint.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct TxReceiptContents<S>(std::marker::PhantomData<S>);

impl<S: Spec> sov_rollup_interface::stf::TxReceiptContents for TxReceiptContents<S> {
    type Skipped = SkippedTxContents<S>;
    type Reverted = RevertedTxContents<S>;
    type Successful = SuccessfulTxContents<S>;
}

/// The result of applying a transaction to the state.
/// This is the value returned when [`process_tx`] succeeds.
/// It contains the new transaction checkpoint, transaction receipt and the amount of gas tokens that the sequencer should be rewarded.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound = "S: Spec")]
pub struct ApplyTxResult<S: Spec> {
    /// Gas consumption.
    pub transaction_consumption: TransactionConsumption<S::Gas>,
    /// The transaction receipt.
    pub receipt: TransactionReceipt<S>,
}

/// Genesis parameters for a blueprint
#[derive(Clone)]
pub struct GenesisParams<RuntimeConfig> {
    /// The runtime genesis parameters
    pub runtime: RuntimeConfig,
}

impl<S, RT> StfBlueprint<S, RT>
where
    S: Spec,
    RT: Runtime<S>,
{
    /// Compute the new state root and change set after running a batch.
    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    #[cfg(feature = "native")]
    pub fn materialize_slot(
        &self,
        should_execute_slot_hooks: bool,
        checkpoint: StateCheckpoint<S::Storage>,
    ) -> (
        <S::Storage as Storage>::Root,
        <S::Storage as Storage>::Witness,
        <S::Storage as Storage>::ChangeSet,
    ) {
        let (next_root_hash, mut state_update, mut accessory_delta, witness, storage) =
            checkpoint.materialize_update();

        if should_execute_slot_hooks {
            self.runtime
                .finalize_hook(&next_root_hash, &mut accessory_delta);
            state_update.add_accessory_items(accessory_delta.freeze());
        }

        let change_set = storage.materialize_changes(&state_update);

        (next_root_hash, witness, change_set)
    }

    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    #[cfg(not(feature = "native"))]
    fn materialize_slot(
        &self,
        checkpoint: StateCheckpoint<S::Storage>,
    ) -> (
        <S::Storage as Storage>::Root,
        <S::Storage as Storage>::Witness,
        <S::Storage as Storage>::ChangeSet,
    ) {
        let (next_root_hash, state_update, _, witness, storage) = checkpoint.materialize_update();

        let change_set = storage.materialize_changes(&state_update);

        (next_root_hash, witness, change_set)
    }
}

impl<S, RT> StateTransitionFunction<S::InnerZkvm, S::OuterZkvm, S::Da> for StfBlueprint<S, RT>
where
    S: Spec,
    RT: Runtime<S>,
    RT: HasKernel<S, BlobType = BlobDataWithId>,
{
    type StateRoot = <S::Storage as Storage>::Root;

    type Address = S::Address;

    type GasPrice = <S::Gas as Gas>::Price;

    type GenesisParams = GenesisParams<<RT as Genesis>::Config>;
    type PreState = S::Storage;
    type ChangeSet = <S::Storage as Storage>::ChangeSet;

    type TxReceiptContents = TxReceiptContents<S>;

    type BatchReceiptContents = BatchSequencerReceipt<S>;

    type StorageProof = StorageProof<<S::Storage as Storage>::Proof>;

    type Witness = <S::Storage as Storage>::Witness;

    type Condition = <S::Da as DaSpec>::ValidityCondition;

    fn init_chain(
        &self,
        genesis_rollup_header: &<S::Da as DaSpec>::BlockHeader,
        validity_condition: &<S::Da as DaSpec>::ValidityCondition,
        pre_state: Self::PreState,
        params: Self::GenesisParams,
    ) -> (Self::StateRoot, Self::ChangeSet) {
        let mut state_checkpoint = StateCheckpoint::new::<S, _>(pre_state, &self.runtime.kernel());

        let mut genesis_accessor =
            state_checkpoint.to_genesis_state_accessor::<RT, S>(&params.runtime);

        if let Err(e) = self.runtime.genesis(
            genesis_rollup_header,
            validity_condition,
            &params.runtime,
            &mut genesis_accessor,
        ) {
            tracing::error!(error = %e, "Runtime initialization must succeed");
            panic!("Runtime initialization must succeed {}", e);
        }

        #[cfg(feature = "native")]
        let (genesis_hash, _, change_set) = self.materialize_slot(true, state_checkpoint);
        #[cfg(not(feature = "native"))]
        let (genesis_hash, _, change_set) = self.materialize_slot(state_checkpoint);

        (genesis_hash, change_set)
    }

    fn apply_slot<'a, I>(
        &self,
        pre_state_root: &Self::StateRoot,
        pre_state: Self::PreState,
        witness: Self::Witness,
        slot_header: &<S::Da as DaSpec>::BlockHeader,
        validity_condition: &<S::Da as DaSpec>::ValidityCondition,
        relevant_blobs: RelevantBlobIters<I>,
        execution_context: ExecutionContext,
    ) -> ApplySlotOutput<S::InnerZkvm, S::OuterZkvm, S::Da, Self>
    where
        I: IntoIterator<Item = &'a mut <S::Da as DaSpec>::BlobTransaction>,
    {
        #[cfg(feature = "native")]
        let start_slot = std::time::Instant::now();
        let mut state =
            StateCheckpoint::with_witness(pre_state.clone(), witness, &self.runtime.kernel());

        let mut kernel = self.runtime.kernel().accessor(&mut state);

        // WARNING: The kernel slot hooks should always be called before the runtime slot hooks.
        // That way the state of the runtime modules is always in sync with the transaction `being executed`.
        //
        // WARNING: The true slot height gets updated in the `ChainState`'s `begin_slot_hook` method.
        // The visible slot height gets updated in the `BlobStorage`'s `get_blobs_for_this_slot` method.
        // Be careful to not respect the call order: the `ChainState` hooks should be called before the `BlobStorage`'s which should be called before the
        // `Runtime`'s slot hooks.
        self.runtime.chain_state().synchronise_chain(
            slot_header,
            validity_condition,
            pre_state_root,
            &mut kernel,
        );

        let visible_hash = self
            .runtime
            .chain_state()
            .current_visible_hash( &mut kernel)
            .expect("The current visible hash should be possible to compute at this point because the chain-state should have synchronized. This is a bug. Please report it.");

        let all_blobs = relevant_blobs
            .batch_blobs
            .into_iter()
            .map(BlobOrigin::Batch)
            .chain(
                relevant_blobs
                    .proof_blobs
                    .into_iter()
                    .map(BlobOrigin::Proof),
            );

        let blob_selector_output = self
            .runtime
            .blob_selector()
            .get_blobs_for_this_slot(all_blobs, &mut kernel)
            .expect("blob selection must succeed, probably serialization failed");

        #[cfg(feature = "native")]
        let blob_selection_time = start_slot.elapsed();

        #[cfg(feature = "native")]
        let should_execute_slot_hooks = blob_selector_output.should_execute_slot_hooks;

        KernelSlotHooks::kernel_begin_slot_hook(
            &self.runtime,
            slot_header,
            validity_condition,
            pre_state_root,
            &mut kernel,
        );

        #[cfg(feature = "native")]
        let visible_height = state.rollup_height_to_access();
        let (total_gas, proof_receipts, batch_receipts, mut state) = self
            .apply_batches_in_user_space(
                blob_selector_output,
                state,
                execution_context,
                visible_hash,
            );

        let mut kernel_state_accessor = self.runtime.kernel().accessor(&mut state);

        self.runtime
            .chain_state()
            .finalise_chain_state(&total_gas, &mut kernel_state_accessor);

        KernelSlotHooks::kernel_end_slot_hook(
            &self.runtime,
            &total_gas,
            &mut kernel_state_accessor,
        );

        #[cfg(not(feature = "native"))]
        let (state_root, witness, change_set) = self.materialize_slot(state);

        #[cfg(feature = "native")]
        let (state_root, witness, change_set) = {
            let slot_finalization_start = std::time::Instant::now();

            // Note the call to materialize slot mixed in with metrics operations here.
            let (state_root, witness, change_set) =
                self.materialize_slot(should_execute_slot_hooks, state);

            let slot_finalization_time = slot_finalization_start.elapsed();
            sov_metrics::track_metrics(|tracker| {
                tracker.track_slot_processing(sov_metrics::SlotProcessingMetrics {
                    blobs_selection_time: blob_selection_time,
                    slot_finalization_time,
                    da_height: slot_header.height(),
                    execution_context,
                    rollup_height: visible_height,
                });
            });
            (state_root, witness, change_set)
        };

        ApplySlotOutput {
            state_root,
            change_set,
            proof_receipts,
            batch_receipts,
            witness,
        }
    }
}

impl<S, RT> StfBlueprint<S, RT>
where
    S: Spec,
    RT: Runtime<S>,
    RT: HasKernel<S>,
{
    /// Run batches provided by the blob selector
    #[allow(clippy::type_complexity)]
    pub fn run_batches_from_blob_selector(
        &self,
        blob_selector_output: BlobSelectorOutput<S, BlobDataWithId<IterableBatchWithId>>,
        state: StateCheckpoint<S::Storage>,
        execution_context: ExecutionContext,
        visible_hash: <<S as Spec>::Storage as Storage>::Root,
    ) -> (
        <S as Spec>::Gas,
        Vec<
            ProofReceipt<
                <S as Spec>::Address,
                <S as Spec>::Da,
                <<S as Spec>::Storage as Storage>::Root,
                StorageProof<<<S as Spec>::Storage as Storage>::Proof>,
            >,
        >,
        Vec<BatchReceipt<S>>,
        StateCheckpoint<S::Storage>,
    ) {
        self.apply_batches_in_user_space(
            blob_selector_output,
            state,
            execution_context,
            visible_hash,
        )
    }

    /// Run the provided sequence of batches, updating the user-space rollup state as we go.
    /// Batches can inject control flow, which will be respected by the runner.
    #[allow(clippy::type_complexity)]
    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    pub fn apply_batches_in_user_space<B: IncrementalBatch<TransactionReceipt<S>, S>>(
        &self,
        blob_selector_output: BlobSelectorOutput<S, BlobDataWithId<B>>,
        mut state: StateCheckpoint<S::Storage>,
        execution_context: ExecutionContext,
        visible_hash: <<S as Spec>::Storage as Storage>::Root,
    ) -> (
        <S as Spec>::Gas,
        Vec<
            ProofReceipt<
                <S as Spec>::Address,
                <S as Spec>::Da,
                <<S as Spec>::Storage as Storage>::Root,
                StorageProof<<<S as Spec>::Storage as Storage>::Proof>,
            >,
        >,
        Vec<BatchReceipt<S>>,
        StateCheckpoint<S::Storage>,
    ) {
        // Note: The gas price should be computed after all the capabilities involving the [`KernelStateAccessor`] to have the
        // most recent version of the virtual rollup height.
        let gas_price = self.runtime.chain_state().base_fee_per_gas(&mut state).expect("The base fee per gas for the current slot should be known at this point! This is a bug. Please report it");

        let visible_height = state.rollup_height_to_access();

        info!(
            blob_count = blob_selector_output.selected_blobs.len(),
            virtual_slot = visible_height,
            "Selected batch(es) for execution in current slot"
        );

        // We run [`SlotHooks::begin_slot_hook`] if the visible height is updated. This is to ensure that we have the
        // following invariant: the `user_space` root only updates when the `virtual_slot_height`` gets increased.
        // If not enforced, this may break soft-confirmations because it will not be possible to deterministically
        // predict the user space state when executing priority blobs.
        #[cfg(feature = "native")]
        let begin_slot_start = std::time::Instant::now();
        if blob_selector_output.should_execute_slot_hooks {
            SlotHooks::begin_slot_hook(&self.runtime, &visible_hash, &mut state);
        }
        #[cfg(feature = "native")]
        let begin_slot_hooks_time = begin_slot_start.elapsed();

        let mut proof_receipts = Vec::new();
        let mut batch_receipts = Vec::new();

        let mut total_gas = S::Gas::zero();
        #[cfg(feature = "native")]
        let blob_processing_start = std::time::Instant::now();
        // TODO: Inject closure to report state changes here
        for (blob_idx, (blob, sender)) in
            blob_selector_output.selected_blobs.into_iter().enumerate()
        {
            match blob {
                BlobDataWithId::Batch(batch) => {
                    #[cfg(feature = "native")]
                    let start_batch_processing = std::time::Instant::now();
                    let batch_id = batch.id();
                    let (batch_receipt, next_checkpoint) = registered::apply_batch::<S, RT, B>(
                        &self.runtime,
                        state,
                        batch,
                        blob_idx,
                        sender,
                        &gas_price,
                        visible_height,
                        execution_context,
                    );
                    // Metrics section
                    #[cfg(feature = "native")]
                    {
                        let processing_time = start_batch_processing.elapsed();
                        let outcome = match &batch_receipt.inner.outcome {
                            sov_modules_api::BatchSequencerOutcome::Executed(_) => {
                                sov_metrics::BatchOutcome::Executed
                            }
                            sov_modules_api::BatchSequencerOutcome::Ignored(_) => {
                                sov_metrics::BatchOutcome::Ignored
                            }
                        };
                        let transactions_count = batch_receipt.tx_receipts.len();
                        sov_metrics::track_metrics(|tracker| {
                            tracker.track_batch_processing(sov_metrics::BatchMetrics {
                                processing_time,
                                transactions_count,
                                outcome,
                            });
                        });
                    };
                    total_gas.combine(&batch_receipt.inner.gas_used);
                    batch_receipts.push(batch_receipt.finalize(batch_id.unwrap_or([0u8; 32])));
                    state = next_checkpoint;
                }
                BlobDataWithId::EmergencyRegistration { tx, id } => {
                    let (batch_receipt, next_checkpoint) = unregistered::apply_batch::<S, RT>(
                        &self.runtime,
                        state,
                        BatchWithSingleTx {
                            auth_input: RT::add_standard_auth(tx),
                            id,
                        },
                        blob_idx,
                        sender,
                        &gas_price,
                        visible_height,
                        execution_context,
                    );

                    total_gas.combine(&batch_receipt.inner.gas_used);
                    batch_receipts.push(batch_receipt);
                    state = next_checkpoint;
                }
                BlobDataWithId::Proof { proof, id } => {
                    let (receipt, next_checkpoint, gas_used) =
                        self.process_proof(id, sender, &gas_price, proof, state);

                    state = next_checkpoint;
                    proof_receipts.push(receipt);
                    total_gas.combine(&gas_used);
                }
            }
        }

        #[cfg(feature = "native")]
        let blob_processing_time = blob_processing_start.elapsed();
        #[cfg(feature = "native")]
        let end_slot_hooks_start = std::time::Instant::now();

        // Note that we run the end-slot hooks even in non-native mode, which is why this can't
        // be a single "native" block
        if blob_selector_output.should_execute_slot_hooks {
            SlotHooks::end_slot_hook(&self.runtime, &mut state);
        }
        #[cfg(feature = "native")]
        {
            let end_slot_hooks_time = end_slot_hooks_start.elapsed();
            sov_metrics::track_metrics(|tracker| {
                tracker.track_user_space_slot_processing(
                    sov_metrics::UserSpaceSlotProcessingMetrics {
                        begin_slot_hooks_time,
                        blobs_processing_time: blob_processing_time,
                        rollup_height: visible_height,
                        execution_context,
                        end_slot_hooks_time,
                    },
                );
            });
        }

        (total_gas, proof_receipts, batch_receipts, state)
    }
}
