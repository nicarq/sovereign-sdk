#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

pub mod kernels;
mod stf_blueprint;

#[cfg(feature = "test-utils")]
mod utils;

#[cfg(all(target_os = "zkvm", feature = "bench"))]
use risc0_cycle_macros::cycle_tracker;
pub use sov_modules_api::batch::Batch;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::hooks::{ApplyBatchHooks, FinalizeHook, SlotHooks, TxHooks};
#[cfg(feature = "mocks")]
use sov_modules_api::runtime::capabilities::mocks::MockKernel;
use sov_modules_api::runtime::capabilities::{Kernel, KernelSlotHooks};
use sov_modules_api::transaction::Transaction;
pub use sov_modules_api::tx_verifier::RawTx;
use sov_modules_api::{
    BasicAddress, BlobReaderTrait, DaSpec, DispatchCall, Gas, GasArray, Genesis, KernelWorkingSet,
    RuntimeEventProcessor, Spec, StateCheckpoint, Zkvm,
};
use sov_modules_core::capabilities::{ContextResolver, GasEnforcer, TransactionDeduplicator};
use sov_modules_core::VersionedStateReadWriter;
pub use sov_rollup_interface::stf::BatchReceipt;
use sov_rollup_interface::stf::{ApplySlotOutput, SlotResult, StateTransitionFunction};
use sov_state::Storage;
pub use stf_blueprint::{apply_tx, ExecutionMode, StfBlueprint};
use tracing::{debug, info};

/// This trait has to be implemented by a runtime in order to be used in `StfBlueprint`.
///
/// The `TxHooks` implementation sets up a transaction context based on the height at which it is
/// to be executed.
pub trait Runtime<S: Spec, Da: DaSpec>:
    DispatchCall<Spec = S>
    + Genesis<Spec = S, Config = Self::GenesisConfig>
    + TxHooks<Spec = S>
    + SlotHooks<Spec = S>
    + FinalizeHook<Spec = S>
    + ApplyBatchHooks<
        Da,
        Spec = S,
        BatchResult = SequencerOutcome<
            <<Da as DaSpec>::BlobTransaction as BlobReaderTrait>::Address,
        >,
    > + Default
    + RuntimeEventProcessor
    + GasEnforcer<S, Da, Tx = Transaction<S>>
    + TransactionDeduplicator<S, Da, Tx = Transaction<S>>
    + ContextResolver<S, Da, Tx = Transaction<S>>
{
    /// GenesisConfig type.
    type GenesisConfig: Send + Sync;

    #[cfg(feature = "native")]
    /// GenesisPaths type.
    type GenesisPaths: Send + Sync;

    #[cfg(feature = "native")]
    /// Default RPC methods.
    fn rpc_methods(
        storage: std::sync::Arc<std::sync::RwLock<S::Storage>>,
    ) -> jsonrpsee::RpcModule<()>;

    #[cfg(feature = "native")]
    /// Reads genesis configs.
    fn genesis_config(
        genesis_paths: &Self::GenesisPaths,
    ) -> Result<Self::GenesisConfig, anyhow::Error>;
}

/// The receipts of all the transactions in a batch.
#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TxEffect {
    /// The transaction was reverted during execution
    Reverted,
    /// Batch was processed successfully.
    Successful,
    /// The transaction was not applied because it did not purchase the minimum required gas.
    /// In this case, the sequencer should be charged the base gas fee.
    InsufficientBaseGas,
    /// The transaction was not applied because it was a duplicate
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
/// Represents the different outcomes that can occur for a sequencer after batch processing.
pub enum SequencerOutcome<A: BasicAddress> {
    /// Sequencer receives reward amount in defined token and can withdraw its deposit. The amount is net of any penalties
    Rewarded(u64),
    /// Sequencer was penalized (on net) for including invalid (but not provably malicious) transactions
    Penalized(u64),
    /// Sequencer loses its deposit and receives no reward
    Slashed {
        /// Reason why sequencer was slashed.
        reason: SlashingReason,
        #[serde(bound(deserialize = ""))]
        /// Sequencer address on DA.
        sequencer_da_address: A,
    },
    /// Batch was ignored, sequencer deposit left untouched.
    Ignored,
}

/// Genesis parameters for a blueprint
pub struct GenesisParams<RuntimeConfig, KernelConfig> {
    /// The runtime genesis parameters
    pub runtime: RuntimeConfig,
    /// The kernel's genesis parameters
    pub kernel: KernelConfig,
}

/// Reason why sequencer was slashed.
#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SlashingReason {
    /// This status indicates problem with batch deserialization.
    InvalidBatchEncoding,
    /// Stateless verification failed, for example deserialized transactions have invalid signatures.
    StatelessVerificationFailed,
    /// This status indicates problem with transaction deserialization.
    InvalidTransactionEncoding,
}

impl<S, RT, Vm, Da, K> StfBlueprint<S, Da, Vm, RT, K>
where
    S: Spec,
    Vm: Zkvm,
    Da: DaSpec,
    RT: Runtime<S, Da>,
    K: KernelSlotHooks<S, Da>,
{
    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    fn begin_slot(
        &self,
        state_checkpoint: &mut StateCheckpoint<S>,
        slot_header: &Da::BlockHeader,
        validity_condition: &Da::ValidityCondition,
        pre_state_root: &<S::Storage as Storage>::Root,
    ) -> <S::Gas as Gas>::Price {
        // WARNING: The kernel slot hooks should always be called before the runtime slot hooks.
        // That way the state of the runtime modules is always in sync with the transaction `being executed`.
        let gas_price = self.kernel.begin_slot_hook(
            slot_header,
            validity_condition,
            pre_state_root,
            state_checkpoint,
        );

        // We build and pass down the VersionedStateReadWriter to the [`begin_slot_hook`] method to have access to context
        // aware information.
        let kernel_working_set = KernelWorkingSet::from_kernel(&self.kernel, state_checkpoint);
        let mut versioned_working_set =
            VersionedStateReadWriter::from_kernel_ws_virtual(kernel_working_set);

        let visible_hash = <S as Spec>::VisibleHash::from(pre_state_root.clone());

        self.runtime
            .begin_slot_hook(visible_hash, &mut versioned_working_set);

        gas_price
    }

    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    fn end_slot(
        &self,
        storage: S::Storage,
        gas_used: &S::Gas,
        mut checkpoint: StateCheckpoint<S>,
    ) -> (
        <S::Storage as Storage>::Root,
        <S::Storage as Storage>::Witness,
        S::Storage,
    ) {
        // Run end_slot_hook
        self.runtime.end_slot_hook(&mut checkpoint);
        self.kernel.end_slot_hook(gas_used, &mut checkpoint);

        let (cache_log, witness) = checkpoint.freeze();

        let (root_hash, state_update) = storage
            .compute_state_update(cache_log, &witness)
            .expect("jellyfish merkle tree update must succeed");

        let visible_root_hash = <S as Spec>::VisibleHash::from(root_hash.clone());

        self.runtime
            .finalize_hook(visible_root_hash, &mut checkpoint.accessory_state());

        let accessory_log = checkpoint.freeze_non_provable();

        storage.commit(&state_update, &accessory_log);

        (root_hash, witness, storage)
    }
}

impl<S, RT, Vm, Da, K> StateTransitionFunction<Vm, Da> for StfBlueprint<S, Da, Vm, RT, K>
where
    S: Spec,
    Da: DaSpec,
    Vm: Zkvm,
    RT: Runtime<S, Da>,
    K: KernelSlotHooks<S, Da, Batch = BatchWithId>,
{
    type StateRoot = <S::Storage as Storage>::Root;

    type GenesisParams =
        GenesisParams<<RT as Genesis>::Config, <K as Kernel<S, Da>>::GenesisConfig>;
    type PreState = S::Storage;
    type ChangeSet = <S::Storage as Storage>::ChangeSet;

    type TxReceiptContents = TxEffect;

    type BatchReceiptContents = SequencerOutcome<<Da::BlobTransaction as BlobReaderTrait>::Address>;

    type Witness = <S::Storage as Storage>::Witness;

    type Condition = Da::ValidityCondition;

    fn init_chain(
        &self,
        pre_state: Self::PreState,
        params: Self::GenesisParams,
    ) -> (Self::StateRoot, Self::ChangeSet) {
        // TODO(@preston-evans98): Get rid of the Clone here by making pre-state read only.
        let mut state_checkpoint = StateCheckpoint::new(pre_state.clone());
        let mut startup_ws = KernelWorkingSet::uninitialized(&mut state_checkpoint);

        // Important! The kernel *must* be initialized before the runtime, since runtime
        // module authors are allowed to depend on the kernel.
        self.kernel
            .genesis(&params.kernel, &mut startup_ws)
            .expect("Kernel initialization must succeed");
        let mut working_set = state_checkpoint.to_revertable(Default::default());
        self.runtime
            .genesis(&params.runtime, &mut working_set)
            .expect("Runtime initialization must succeed");

        let mut checkpoint = working_set.checkpoint().0;
        let (log, witness) = checkpoint.freeze();

        let (genesis_hash, state_update) = pre_state
            .compute_state_update(log, &witness)
            .expect("Storage update must succeed");

        let visible_genesis_hash = <S as Spec>::VisibleHash::from(genesis_hash.clone());

        self.runtime
            .finalize_hook(visible_genesis_hash, &mut checkpoint.accessory_state());

        let accessory_log = checkpoint.freeze_non_provable();
        // HACK: Drop the old checkpoint to ensure that it's RC is not active during commit.
        // This will be resolved as part of https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/132
        drop(checkpoint);

        // TODO: Commit here for now, but probably this can be done outside of STF
        // TODO: Commit is fine
        pre_state.commit(&state_update, &accessory_log);

        (genesis_hash, pre_state.to_change_set())
    }

    fn apply_slot<'a, I>(
        &self,
        pre_state_root: &Self::StateRoot,
        pre_state: Self::PreState,
        witness: Self::Witness,
        slot_header: &Da::BlockHeader,
        validity_condition: &Da::ValidityCondition,
        blobs: I,
    ) -> ApplySlotOutput<Vm, Da, Self>
    where
        I: IntoIterator<Item = &'a mut Da::BlobTransaction>,
    {
        let mut checkpoint = StateCheckpoint::with_witness(pre_state.clone(), witness);
        let gas_price = self.begin_slot(
            &mut checkpoint,
            slot_header,
            validity_condition,
            pre_state_root,
        );

        let mut kernel_working_set = KernelWorkingSet::from_kernel(&self.kernel, &mut checkpoint);
        let visible_height = kernel_working_set.virtual_slot();
        let selected_batches = self
            .kernel
            .get_batches_for_this_slot(blobs, &mut kernel_working_set)
            .expect("blob selection must succeed, probably serialization failed");

        info!(
            batches_count = selected_batches.len(),
            virtual_slot = visible_height,
            true_slot = kernel_working_set.current_slot(),
            "Selected batch(es) for execution in current slot"
        );

        let mut batch_receipts = vec![];

        let mut total_gas = S::Gas::zero();
        for (blob_idx, (batch, sender)) in selected_batches.into_iter().enumerate() {
            let (apply_blob_result, next_checkpoint, gas_used) =
                self.apply_batch(checkpoint, batch, &sender, &gas_price, visible_height);
            checkpoint = next_checkpoint;
            let batch_receipt = apply_blob_result.unwrap_or_else(Into::into);
            info!(
                blob_idx,
                blob_hash = hex::encode(batch_receipt.batch_hash),
                %sender,
                num_txs = batch_receipt.tx_receipts.len(),
                sequencer_outcome = ?batch_receipt.inner,
                ?gas_used,
                "Applied blob and got the sequencer outcome"
            );
            for (i, tx_receipt) in batch_receipt.tx_receipts.iter().enumerate() {
                debug!(
                    tx_idx = i,
                    tx_hash = hex::encode(tx_receipt.tx_hash),
                    receipt = ?tx_receipt.receipt,
                    "Tx receipt"
                );
            }
            batch_receipts.push(batch_receipt);
            total_gas.combine(&gas_used);
        }

        let (state_root, witness, storage) = self.end_slot(pre_state, &total_gas, checkpoint);
        SlotResult {
            state_root,
            change_set: storage.to_change_set(),
            batch_receipts,
            witness,
        }
    }
}
