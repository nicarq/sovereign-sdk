#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
mod stf_blueprint;
use sequencer_mode::{registered, unregistered};
use serde::{Deserialize, Serialize};
use sov_modules_api::{BatchSequencerReceipt, VersionReader};
mod proof_processing;
mod sequencer_mode;
#[cfg(feature = "test-utils")]
mod utils;
/// We export the `apply_tx` function to use inside the simulation endpoints.
pub use sequencer_mode::apply_tx;
pub use sequencer_mode::common::{get_gas_used, AuthTxOutput, BatchReceipt, TransactionReceipt};
pub use sequencer_mode::registered::{authenticate_tx, process_tx, PreExecError};
#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_cycle_utils::macros::cycle_tracker;
use sov_modules_api::capabilities::{BlobOrigin, HasCapabilities, TransactionAuthenticator};
use sov_modules_api::hooks::{ApplyBatchHooks, FinalizeHook, SlotHooks, TxHooks};
use sov_modules_api::runtime::capabilities::KernelSlotHooks;
use sov_modules_api::transaction::TransactionConsumption;
pub use sov_modules_api::{BatchWithId, BlobData};
use sov_modules_api::{
    BlobDataWithId, DaSpec, DispatchCall, Error, ExecutionContext, Gas, GasArray, Genesis,
    RuntimeEventProcessor, Spec, StateCheckpoint, WorkingSet,
};
use sov_rollup_interface::da::RelevantBlobIters;
use sov_rollup_interface::stf::{ApplySlotOutput, StateTransitionFunction};
use sov_state::storage::StateUpdate;
use sov_state::{Storage, StorageProof};
pub use stf_blueprint::StfBlueprint;
use thiserror::Error;
use tracing::info;

use crate::unregistered::BatchWithSingleTx;
/// This trait has to be implemented by a runtime in order to be used in `StfBlueprint`.
///
/// The `TxHooks` implementation sets up a transaction context based on the height at which it is
/// to be executed.
pub trait Runtime<S: Spec>:
    DispatchCall<Spec = S>
    + HasCapabilities<S>
    + TransactionAuthenticator<
        S,
        Decodable = <Self as DispatchCall>::Decodable,
        AuthorizationData = <Self as HasCapabilities<S>>::AuthorizationData,
    > + Genesis<Spec = S, Config = Self::GenesisConfig>
    + TxHooks<Spec = S, TxState = WorkingSet<S>>
    + SlotHooks<Spec = S>
    + FinalizeHook<Spec = S>
    + ApplyBatchHooks<Spec = S, BatchResult = BatchSequencerReceipt<<S as Spec>::Da>>
    + Default
    + RuntimeEventProcessor
{
    /// GenesisConfig type.
    type GenesisConfig: Send + Sync;

    /// GenesisPaths type.
    #[cfg(feature = "native")]
    type GenesisPaths: Send + Sync;

    /// Default RPC methods and Axum router.
    #[cfg(feature = "native")]
    fn endpoints(storage: sov_modules_api::rest::ApiState<S>) -> RuntimeEndpoints;

    /// Reads genesis configs.
    #[cfg(feature = "native")]
    fn genesis_config(genesis_paths: &Self::GenesisPaths) -> anyhow::Result<Self::GenesisConfig>;
}

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
    type Reverted = RevertedTxContents<S>;
    type Skipped = SkippedTxContents<S>;
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
pub struct GenesisParams<RuntimeConfig> {
    /// The runtime genesis parameters
    pub runtime: RuntimeConfig,
}

impl<S, RT, K> StfBlueprint<S, RT, K>
where
    S: Spec,
    RT: Runtime<S>,
    K: KernelSlotHooks<S>,
{
    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    fn begin_slot(
        &self,
        state: &mut StateCheckpoint<S::Storage>,
        _slot_header: &<S::Da as DaSpec>::BlockHeader,
        _validity_condition: &<S::Da as DaSpec>::ValidityCondition,
        visible_hash: &<<S as Spec>::Storage as Storage>::Root,
    ) {
        self.runtime.begin_slot_hook(visible_hash, state);
    }

    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    fn end_slot(
        &self,
        storage: S::Storage,
        gas_used: &S::Gas,
        mut checkpoint: StateCheckpoint<S::Storage>,
    ) -> (
        <S::Storage as Storage>::Root,
        <S::Storage as Storage>::Witness,
        <S::Storage as Storage>::ChangeSet,
    ) {
        // Run end_slot_hook
        self.runtime.end_slot_hook(&mut checkpoint);

        let mut kernel_state_accessor = self.kernel.accessor(&mut checkpoint);

        self.kernel
            .end_slot_hook(gas_used, &mut kernel_state_accessor);

        let (cache_log, mut accessory_delta, witness) = checkpoint.freeze();

        let (next_root_hash, mut state_update) = storage
            .compute_state_update(cache_log, &witness)
            .expect("jellyfish merkle tree update must succeed");

        self.runtime
            .finalize_hook(&next_root_hash, &mut accessory_delta);

        state_update.add_accessory_items(accessory_delta.freeze());
        let change_set = storage.materialize_changes(&state_update);

        (next_root_hash, witness, change_set)
    }
}

impl<S, RT, Da, K> StateTransitionFunction<S::InnerZkvm, S::OuterZkvm, Da>
    for StfBlueprint<S, RT, K>
where
    S: Spec<Da = Da>,
    Da: DaSpec,
    RT: Runtime<S>,
    K: KernelSlotHooks<S, BlobType = BlobDataWithId>,
{
    type StateRoot = <S::Storage as Storage>::Root;

    type Address = S::Address;

    type GasPrice = <S::Gas as Gas>::Price;

    type GenesisParams = GenesisParams<<RT as Genesis>::Config>;
    type PreState = S::Storage;
    type ChangeSet = <S::Storage as Storage>::ChangeSet;

    type TxReceiptContents = TxReceiptContents<S>;

    type BatchReceiptContents = BatchSequencerReceipt<Da>;

    type StorageProof = StorageProof<<S::Storage as Storage>::Proof>;

    type Witness = <S::Storage as Storage>::Witness;

    type Condition = Da::ValidityCondition;

    fn init_chain(
        &self,
        pre_state: Self::PreState,
        params: Self::GenesisParams,
    ) -> (Self::StateRoot, Self::ChangeSet) {
        // TODO(@preston-evans98): Get rid of the Clone here by making pre-state read only.
        let mut state_checkpoint =
            StateCheckpoint::new::<S, K>(pre_state.clone(), &Default::default());

        let mut genesis_accessor =
            state_checkpoint.to_genesis_state_accessor::<RT, S>(&params.runtime);

        if let Err(e) = self.runtime.genesis(&params.runtime, &mut genesis_accessor) {
            tracing::error!(error = %e, "Runtime initialization must succeed");
            panic!("Runtime initialization must succeed {}", e);
        }

        let (log, mut accessory_delta, witness) = state_checkpoint.freeze();

        let (genesis_hash, mut state_update) = pre_state
            .compute_state_update(log, &witness)
            .expect("Storage update must succeed");

        self.runtime
            .finalize_hook(&genesis_hash, &mut accessory_delta);

        state_update.add_accessory_items(accessory_delta.freeze());

        let change_set = pre_state.materialize_changes(&state_update);

        (genesis_hash, change_set)
    }

    fn apply_slot<'a, I>(
        &self,
        pre_state_root: &Self::StateRoot,
        pre_state: Self::PreState,
        witness: Self::Witness,
        slot_header: &Da::BlockHeader,
        validity_condition: &Da::ValidityCondition,
        relevant_blobs: RelevantBlobIters<I>,
        execution_context: ExecutionContext,
    ) -> ApplySlotOutput<S::InnerZkvm, S::OuterZkvm, Da, Self>
    where
        I: IntoIterator<Item = &'a mut Da::BlobTransaction>,
    {
        let mut state = StateCheckpoint::with_witness(pre_state.clone(), witness, &self.kernel);

        let mut kernel_accessor = self.kernel.accessor(&mut state);

        // TODO(@theochap, `https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1372`): this should be a capability.
        //
        // WARNING: The kernel slot hooks should always be called before the runtime slot hooks.
        // That way the state of the runtime modules is always in sync with the transaction `being executed`.
        //
        // WARNING: The true slot height gets updated in the `ChainState`'s `begin_slot_hook` method.
        // The visible slot height gets updated in the `BlobStorage`'s `get_blobs_for_this_slot` method.
        // Be careful to not respect the call order: the `ChainState` hooks should be called before the `BlobStorage`'s which should be called before the
        // `Runtime`'s slot hooks.
        let visible_state_root = self.kernel.begin_slot_hook(
            slot_header,
            validity_condition,
            pre_state_root,
            &mut kernel_accessor,
        );

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

        let selected_blobs = self
            .kernel
            .get_blobs_for_this_slot(all_blobs, &mut kernel_accessor)
            .expect("blob selection must succeed, probably serialization failed");

        self.begin_slot(
            &mut state,
            slot_header,
            validity_condition,
            &visible_state_root,
        );

        // Note: The gas price should be computed after all the capabilities involving the [`KernelStateAccessor`] to have the
        // most recent version of the virtual slot number.
        let gas_price = self.kernel.base_fee_per_gas(&mut state);

        let visible_height = state.rollup_height_to_access();

        if !selected_blobs.is_empty() {
            info!(
                blob_count = selected_blobs.len(),
                virtual_slot = visible_height,
                "Selected batch(es) for execution in current slot"
            );
        }

        let mut proof_receipts = Vec::new();
        let mut batch_receipts = vec![];

        let mut total_gas = S::Gas::zero();
        for (blob_idx, (blob, sender)) in selected_blobs.into_iter().enumerate() {
            match blob.data {
                BlobData::Batch(batch) => {
                    let (batch_receipt, next_checkpoint, gas_used) =
                        registered::apply_batch::<S, RT, K>(
                            &self.runtime,
                            state,
                            BatchWithId { batch, id: blob.id },
                            blob_idx,
                            sender,
                            &gas_price,
                            visible_height,
                            execution_context,
                        );

                    batch_receipts.push(batch_receipt);
                    total_gas.combine(&gas_used);
                    state = next_checkpoint;
                }
                BlobData::EmergencyRegistration(tx) => {
                    let (batch_receipt, next_checkpoint, gas_used) =
                        unregistered::apply_batch::<S, RT, K>(
                            &self.runtime,
                            state,
                            BatchWithSingleTx {
                                fully_baked_tx: RT::encode_with_standard_auth(tx),
                                id: blob.id,
                            },
                            blob_idx,
                            sender,
                            &gas_price,
                            visible_height,
                            execution_context,
                        );

                    batch_receipts.push(batch_receipt);
                    total_gas.combine(&gas_used);
                    state = next_checkpoint;
                }
                BlobData::Proof(proof) => {
                    let (receipt, next_checkpoint, gas_used) =
                        self.process_proof(blob.id, sender, &gas_price, proof, state);

                    state = next_checkpoint;
                    proof_receipts.push(receipt);
                    total_gas.combine(&gas_used);
                }
            }
        }

        let (state_root, witness, change_set) = self.end_slot(pre_state, &total_gas, state);
        ApplySlotOutput {
            state_root,
            change_set,
            proof_receipts,
            batch_receipts,
            witness,
        }
    }
}

/// The return type of [`Runtime::endpoints`].
#[cfg(feature = "native")]
pub struct RuntimeEndpoints {
    /// The [`axum::Router`] for the runtime's HTTP server.
    pub axum_router: axum::Router<()>,
    /// A [`jsonrpsee::RpcModule`] for the runtime's JSON-RPC server.
    pub jsonrpsee_module: jsonrpsee::RpcModule<()>,
}

#[cfg(feature = "native")]
impl Default for RuntimeEndpoints {
    fn default() -> Self {
        Self {
            axum_router: Default::default(),
            jsonrpsee_module: jsonrpsee::RpcModule::new(()),
        }
    }
}
