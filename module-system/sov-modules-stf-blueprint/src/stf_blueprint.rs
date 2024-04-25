use std::marker::PhantomData;

#[cfg(feature = "native")]
use borsh::BorshSerialize;
#[cfg(all(target_os = "zkvm", feature = "bench"))]
use risc0_cycle_macros::cycle_tracker;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::runtime::capabilities::KernelSlotHooks;
use sov_modules_api::transaction::{
    AuthenticatedTransactionAndRawHash, AuthenticatedTransactionData,
};
use sov_modules_api::{Context, DaSpec, DispatchCall, Gas, GasArray, Spec, StateCheckpoint};
use sov_modules_core::capabilities::AuthenticationError;
use sov_modules_core::WorkingSet;
use sov_rollup_interface::stf::{BatchReceipt, StoredEvent, TransactionReceipt};
use tracing::{debug, error};

use crate::{Runtime, SequencerOutcome, SlashingReason, TxEffect};

type ApplyBatchResult<T> = Result<T, ApplyBatchError>;

#[allow(type_alias_bounds)]
pub(crate) type ApplyBatch = ApplyBatchResult<BatchReceipt<SequencerOutcome, TxEffect>>;

/// An implementation of the
/// [`StateTransitionFunction`](sov_rollup_interface::stf::StateTransitionFunction)
/// that is specifically designed to work with the module-system.
pub struct StfBlueprint<S: Spec, Da: DaSpec, Vm, RT: Runtime<S, Da>, K: KernelSlotHooks<S, Da>> {
    /// State storage used by the rollup.
    /// The runtime includes all the modules that the rollup supports.
    pub(crate) runtime: RT,
    pub(crate) kernel: K,
    phantom_context: PhantomData<S>,
    phantom_vm: PhantomData<Vm>,
    phantom_da: PhantomData<Da>,
}

pub(crate) enum ApplyBatchError {
    // Contains batch hash
    Ignored([u8; 32]),
    Slashed {
        // Contains batch hash
        hash: [u8; 32],
        reason: SlashingReason,
    },
}

/// The mode in which a transaction executes
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum ExecutionMode {
    /// Normal transaction execution, used while validating/syncing the chain
    Normal,
    /// Speculative execution, used during block building
    Speculative,
}

impl From<ApplyBatchError> for BatchReceipt<SequencerOutcome, TxEffect> {
    fn from(value: ApplyBatchError) -> Self {
        match value {
            ApplyBatchError::Ignored(hash) => BatchReceipt {
                batch_hash: hash,
                tx_receipts: Vec::new(),
                inner: SequencerOutcome::Ignored,
                gas_price: Vec::new(),
            },
            ApplyBatchError::Slashed { hash, reason } => BatchReceipt {
                batch_hash: hash,
                tx_receipts: Vec::new(),
                inner: SequencerOutcome::Slashed(reason),
                gas_price: Vec::new(),
            },
        }
    }
}

impl<S, Vm, Da, RT, K> Default for StfBlueprint<S, Da, Vm, RT, K>
where
    S: Spec,
    Da: DaSpec,
    RT: Runtime<S, Da>,
    K: KernelSlotHooks<S, Da>,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<S, Vm, Da, RT, K> StfBlueprint<S, Da, Vm, RT, K>
where
    S: Spec,
    Da: DaSpec,
    RT: Runtime<S, Da>,
    K: KernelSlotHooks<S, Da>,
{
    /// [`StfBlueprint`] constructor.
    pub fn new() -> Self {
        Self {
            runtime: RT::default(),
            kernel: K::default(),
            phantom_context: PhantomData,
            phantom_vm: PhantomData,
            phantom_da: PhantomData,
        }
    }

    #[tracing::instrument(skip_all, name = "StfBlueprint::apply_batch")]
    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    pub(crate) fn apply_proof(
        &self,
        checkpoint: StateCheckpoint<S>,
        _batch: &mut <Da as DaSpec>::BlobTransaction,
        _gas_price: &<S::Gas as Gas>::Price,
    ) -> StateCheckpoint<S> {
        checkpoint
    }

    #[tracing::instrument(skip_all, name = "StfBlueprint::apply_batch")]
    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    pub(crate) fn apply_batch(
        &self,
        mut checkpoint: StateCheckpoint<S>,
        mut batch: BatchWithId,
        sender: &Da::Address,
        gas_price: &<S::Gas as Gas>::Price,
        height: u64,
    ) -> (ApplyBatch, StateCheckpoint<S>, S::Gas) {
        debug!(
            sequencer_da_address = %sender,
            ?gas_price,
            "Applying a batch"
        );

        // ApplyBlobHook: begin
        if let Err(e) = self
            .runtime
            .begin_batch_hook(&mut batch, sender, &mut checkpoint)
        {
            error!(
                error = %e,
                batch_id = hex::encode(batch.id),
                "Error: The batch was rejected by the 'begin_batch_hook' hook. Skipping batch without slashing the sequencer",
            );

            return (
                Err(ApplyBatchError::Ignored(batch.id)),
                checkpoint,
                S::Gas::zero(),
            );
        }

        let batch_id = batch.id;

        let (txs, messages) = match self.pre_process_batch(batch) {
            Ok((txs, messages)) => (txs, messages),
            Err(reason) => {
                // Explicitly revert on slashing, even though nothing has changed in pre_process.
                let sequencer_da_address = sender;
                let sequencer_outcome = SequencerOutcome::Slashed(reason);
                self.runtime.end_batch_hook(
                    sequencer_outcome,
                    sequencer_da_address,
                    &mut checkpoint,
                );

                return (
                    Err(ApplyBatchError::Slashed {
                        hash: batch_id,
                        reason,
                    }),
                    checkpoint,
                    S::Gas::zero(),
                );
            }
        };

        // Sanity check after pre-processing
        assert_eq!(
            txs.len(),
            messages.len(),
            "Error in preprocessing batch, there should be same number of txs and messages"
        );

        let mut sequencer_reward = 0;

        let mut tx_receipts = Vec::with_capacity(txs.len());

        let (mut checkpoint, gas_used) = self.apply_txs(
            txs,
            messages,
            &mut tx_receipts,
            checkpoint,
            sender,
            &mut sequencer_reward,
            gas_price,
            height,
        );

        let sequencer_outcome = if sequencer_reward >= 0 {
            SequencerOutcome::Rewarded(sequencer_reward.unsigned_abs())
        } else {
            SequencerOutcome::Penalized(sequencer_reward.unsigned_abs())
        };

        self.runtime
            .end_batch_hook(sequencer_outcome.clone(), sender, &mut checkpoint);

        (
            Ok(BatchReceipt {
                batch_hash: batch_id,
                tx_receipts,
                inner: sequencer_outcome,
                gas_price: gas_price.to_vec(),
            }),
            checkpoint,
            gas_used,
        )
    }

    // Do all stateless checks and data formatting, that can be results in sequencer slashing
    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    fn pre_process_batch(
        &self,
        batch: BatchWithId,
    ) -> Result<
        (
            Vec<AuthenticatedTransactionAndRawHash<S>>,
            Vec<<RT as DispatchCall>::Decodable>,
        ),
        SlashingReason,
    > {
        let raw_txs = batch.txs;
        let mut txs = Vec::with_capacity(raw_txs.len());
        let mut messages = Vec::with_capacity(raw_txs.len());

        debug!(txs_num = raw_txs.len(), "Verifying transactions");
        for raw_tx in raw_txs.iter() {
            match self.runtime.authenticate(raw_tx) {
                Ok((tx, decodable)) => {
                    txs.push(tx);
                    messages.push(decodable);
                }
                Err(AuthenticationError::SigVerificationFailed(e)) => {
                    error!("Stateless verification error - the sequencer included a transaction which was known to be invalid. {}\n", e);
                    return Err(SlashingReason::StatelessVerificationFailed);
                }
                Err(AuthenticationError::MessageDecodingFailed(e, raw_tx_hash)) => {
                    error!("Tx 0x{} decoding error: {}", hex::encode(raw_tx_hash), e);
                    return Err(SlashingReason::InvalidTransactionEncoding);
                }
            }
        }

        Ok((txs, messages))
    }

    #[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
    #[allow(clippy::too_many_arguments)]
    fn apply_txs(
        &self,
        txs: Vec<AuthenticatedTransactionAndRawHash<S>>,
        messages: Vec<<RT as DispatchCall>::Decodable>,
        tx_receipts: &mut Vec<TransactionReceipt<TxEffect>>,
        mut batch_workspace: StateCheckpoint<S>,
        sequencer: &Da::Address,
        sequencer_reward: &mut i64,
        gas_price: &<S::Gas as Gas>::Price,
        height: u64,
    ) -> (StateCheckpoint<S>, S::Gas) {
        let mut gas_used = S::Gas::zero();
        for (tx, msg) in txs.into_iter().zip(messages.into_iter()) {
            let (next_workspace, receipt) = apply_tx(
                &self.runtime,
                &tx,
                msg,
                batch_workspace,
                sequencer,
                sequencer_reward,
                ExecutionMode::Normal,
                gas_price,
                height,
            );
            batch_workspace = next_workspace;
            gas_used.combine(&S::Gas::from_slice(&receipt.gas_used));
            tx_receipts.push(receipt);
        }

        (batch_workspace, gas_used)
    }
}

/// Applies a single transaction to the current state. In normal execution, we commit twice times execution:
/// 1. After the pre-dispatch hook. This ensures that the gas charges are paid even if the transaction fails later during execution
/// 2. After the post-dispatch hook. This ensures that the transaction can be reverted by the post-dispatch hook if desired.
#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
#[allow(clippy::too_many_arguments)]
pub fn apply_tx<S, RT, Da>(
    runtime: &RT,
    tx: &AuthenticatedTransactionAndRawHash<S>,
    message: <RT as DispatchCall>::Decodable,
    mut state_checkpoint: StateCheckpoint<S>,
    sequencer: &Da::Address,
    sequencer_reward: &mut i64,
    execution_mode: ExecutionMode,
    gas_price: &<S::Gas as Gas>::Price,
    height: u64,
) -> (StateCheckpoint<S>, TransactionReceipt<TxEffect>)
where
    S: Spec,
    Da: DaSpec,
    RT: Runtime<S, Da>,
{
    let raw_tx_hash = &tx.raw_tx_hash;
    let tx = &tx.authenticated_tx;

    let ctx = runtime.resolve_context(tx, sequencer, height, &mut state_checkpoint);
    // Check that the transaction isn't a duplicate
    if let Err(e) = runtime.check_uniqueness(tx, &ctx, &mut state_checkpoint) {
        error!(
            "Tx 0x{} was rejected by the 'check_uniqueness' hook: {}",
            hex::encode(raw_tx_hash),
            e
        );
        if execution_mode != ExecutionMode::Speculative {
            *sequencer_reward = sequencer_reward.saturating_sub(
                tx.gas_fixed_cost()
                    .value(gas_price)
                    .try_into()
                    .unwrap_or(i64::MAX),
            );
        }

        return (
            state_checkpoint,
            TransactionReceipt {
                tx_hash: *raw_tx_hash,
                body_to_save: None,
                events: vec![],
                receipt: TxEffect::Duplicate,
                gas_used: tx.gas_fixed_cost().to_vec(),
            },
        );
    }

    let mut working_set = match runtime.try_reserve_gas(tx, &ctx, gas_price, state_checkpoint) {
        Ok(working_set) => working_set,
        Err(checkpoint) => {
            error!(
                "Tx 0x{} was rejected by the 'try_reserve_gas' hook.",
                hex::encode(raw_tx_hash),
            );
            if execution_mode != ExecutionMode::Speculative {
                *sequencer_reward = sequencer_reward.saturating_sub(
                    tx.gas_fixed_cost()
                        .value(gas_price)
                        .try_into()
                        .unwrap_or(i64::MAX),
                );
            }
            return (
                checkpoint,
                TransactionReceipt {
                    tx_hash: *raw_tx_hash,
                    body_to_save: None,
                    events: vec![],
                    receipt: TxEffect::InsufficientBaseGas,
                    gas_used: tx.gas_fixed_cost().to_vec(),
                },
            );
        }
    };
    let initial_funds = working_set.gas_remaining_funds();

    let tx_result = attempt_tx(runtime, tx, message, &mut working_set, &ctx);
    let (mut checkpoint, receipt, mut gas_meter) = match tx_result {
        Ok(_) => {
            let (checkpoint, gas_meter, events) = working_set.checkpoint();

            (
                checkpoint,
                TransactionReceipt {
                    tx_hash: *raw_tx_hash,
                    body_to_save: None,
                    events: convert_to_runtime_events::<S, RT, Da>(events),
                    receipt: TxEffect::Successful,
                    gas_used: gas_meter.gas_used().to_vec(),
                },
                gas_meter,
            )
        }
        Err(e) => {
            error!(
                "Tx 0x{} was reverted error: {}",
                hex::encode(raw_tx_hash),
                e
            );
            // the transaction causing invalid state transition is reverted,
            // but we don't slash and continue processing remaining transactions.
            // working_set.revert_in_place();
            let (checkpoint, gas_meter) = working_set.revert();

            (
                checkpoint,
                TransactionReceipt {
                    tx_hash: *raw_tx_hash,
                    body_to_save: None,
                    events: vec![], // As in Ethereum, reverted transactions don't emit events
                    receipt: TxEffect::Reverted,
                    gas_used: gas_meter.gas_used().to_vec(),
                },
                gas_meter,
            )
        }
    };

    // If the transaction failed in speculative mode, we act as though it never happened.
    if execution_mode == ExecutionMode::Speculative && receipt.receipt != TxEffect::Successful {
        tracing::info!(
            "Tx 0x{} was unsuccessful: {:?}. Undoing all effects.",
            hex::encode(raw_tx_hash),
            receipt.receipt
        );
        gas_meter.set_gas_funds(initial_funds);
        runtime.refund_remaining_gas(tx, &ctx, &gas_meter, &mut checkpoint);
        return (checkpoint, receipt);
    }

    runtime.mark_tx_attempted(tx, sequencer, &mut checkpoint);

    runtime.refund_remaining_gas(tx, &ctx, &gas_meter, &mut checkpoint);

    let gas_reward = tx
        .max_priority_fee_per_gas()
        .apply(gas_meter.gas_used().value(gas_meter.gas_price()))
        .unwrap_or(u64::MAX)
        .try_into()
        .unwrap_or(i64::MAX);
    *sequencer_reward = sequencer_reward.saturating_add(gas_reward);
    debug!(
        tx_hash =
        hex::encode(raw_tx_hash),
        receipt= ?receipt.receipt,
        %gas_reward,
    "Sequencer reward has be updated after tx execution",
        );

    (checkpoint, receipt)
}

fn attempt_tx<S, RT, Da>(
    runtime: &RT,
    tx: &AuthenticatedTransactionData<S>,
    message: <RT as DispatchCall>::Decodable,
    working_set: &mut WorkingSet<S>,
    ctx: &Context<S>,
) -> Result<(), anyhow::Error>
where
    S: Spec,
    Da: DaSpec,
    RT: Runtime<S, Da>,
{
    working_set.charge_gas(&tx.gas_fixed_cost())?;
    // TODO(@preston-evans98): Consider moving this before the gas resolution.
    // Also consider whether this needs to be fallible

    runtime.pre_dispatch_tx_hook(tx, working_set)?;

    runtime
        .dispatch_call(message, working_set, ctx)
        .map_err(Into::<anyhow::Error>::into)?;
    runtime.post_dispatch_tx_hook(tx, ctx, working_set)?;

    Ok(())
}

#[cfg(feature = "native")]
pub(crate) fn convert_to_runtime_events<S, RT, Da>(
    events: Vec<sov_modules_core::TypedEvent>,
) -> Vec<StoredEvent>
where
    S: Spec,
    Da: DaSpec,
    RT: Runtime<S, Da>,
{
    events
        .into_iter()
        .map(|typed_event| {
            // This seems to be needed because doing `&typed_event.event_key().to_vec()`
            // directly as the first function param to Event::new() is running into a linter bug
            // where it thinks that the to_vec is not necessary.
            // (probably due to the borrow and move in the same statement)
            // https://github.com/rust-lang/rust-clippy/issues/12098
            let key = typed_event.event_key().to_vec();
            StoredEvent::new(
                &key,
                &<RT as sov_modules_api::RuntimeEventProcessor>::convert_to_runtime_event(
                    typed_event,
                )
                .expect("Unknown event type")
                .try_to_vec()
                .expect("unable to serialize event"),
            )
        })
        .collect()
}

#[cfg(not(feature = "native"))]
fn convert_to_runtime_events<S, RT, Da>(
    _events: Vec<sov_modules_core::TypedEvent>,
) -> Vec<StoredEvent>
where
    S: Spec,
    Da: DaSpec,
    RT: Runtime<S, Da>,
{
    Vec::new() // Return an empty vector
}
