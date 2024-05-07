use std::cmp::min;
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
use sov_modules_api::{
    Context, DaSpec, DispatchCall, Gas, GasArray, GasMeter, Spec, StateCheckpoint,
};
use sov_modules_core::capabilities::{AuthenticationError, FatalError};
use sov_modules_core::{GasTracker, WorkingSet};
use sov_rollup_interface::stf::{BatchReceipt, StoredEvent, TransactionReceipt};
use tracing::{debug, error, info};

use crate::{Runtime, SequencerOutcome, TxEffect};

type ApplyBatchResult<T> = Result<T, ApplyBatchError<TxEffect>>;

#[allow(type_alias_bounds)]
pub(crate) type ApplyBatch = ApplyBatchResult<BatchReceipt<SequencerOutcome, TxEffect>>;

/// An implementation of the
/// [`StateTransitionFunction`](sov_rollup_interface::stf::StateTransitionFunction)
/// that is specifically designed to work with the module-system.
pub struct StfBlueprint<S: Spec, Da: DaSpec, RT: Runtime<S, Da>, K: KernelSlotHooks<S, Da>> {
    /// State storage used by the rollup.
    /// The runtime includes all the modules that the rollup supports.
    pub(crate) runtime: RT,
    pub(crate) kernel: K,
    phantom_context: PhantomData<S>,
    phantom_da: PhantomData<Da>,
}

pub(crate) enum ApplyBatchError<TxReceiptContents> {
    // Contains batch hash
    Ignored([u8; 32]),
    Slashed {
        // Contains batch hash
        hash: [u8; 32],
        tx_receipts: Vec<TransactionReceipt<TxReceiptContents>>,
        // TODO(@theochap) `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/595>`: change to `S::Gas`
        gas_price: Vec<u64>,
        reason: FatalError,
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

impl From<ApplyBatchError<TxEffect>> for BatchReceipt<SequencerOutcome, TxEffect> {
    fn from(value: ApplyBatchError<TxEffect>) -> Self {
        match value {
            ApplyBatchError::Ignored(hash) => BatchReceipt {
                batch_hash: hash,
                tx_receipts: Vec::new(),
                inner: SequencerOutcome::Ignored,
                gas_price: Vec::new(),
            },
            ApplyBatchError::Slashed {
                hash,
                tx_receipts,
                gas_price,
                reason,
            } => BatchReceipt {
                batch_hash: hash,
                tx_receipts,
                inner: SequencerOutcome::Slashed(reason),
                gas_price,
            },
        }
    }
}

impl<S, Da, RT, K> Default for StfBlueprint<S, Da, RT, K>
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

impl<S, Da, RT, K> StfBlueprint<S, Da, RT, K>
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
            phantom_da: PhantomData,
        }
    }

    #[tracing::instrument(skip_all, name = "StfBlueprint::apply_proof")]
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
            batch_id = hex::encode(batch.id),
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

        let raw_txs = batch.txs;

        let mut tx_receipts = Vec::with_capacity(raw_txs.len());
        let mut gas_used = S::Gas::zero();
        let mut accumulated_reward = 0;

        debug!(
            batch_id = hex::encode(batch.id),
            txs_num = raw_txs.len(),
            "Verifying & executing transactions"
        );

        for raw_tx in raw_txs.iter() {
            let sequencer_da_address = sender;
            // Checks the sequencer balance before the transaction is executed.
            // If the sequencer balance is not high enough, the transaction is rejected.
            if let Err(e) = self
                .runtime
                .authorize_sequencer(sequencer_da_address, &mut checkpoint)
            {
                error!(
                    error = %e,
                    batch_id = hex::encode(batch.id),
                    sequencer_da_address = %sequencer_da_address,
                    "Error: The batch was rejected by the 'authorize_sequencer' capability. Skipping batch without slashing the sequencer",
                );

                break;
            };

            match self.runtime.authenticate(raw_tx) {
                Err(AuthenticationError::FatalError(err)) => {
                    error!(err=%err, "Tx authentication failed");

                    self.runtime.end_batch_hook(
                        SequencerOutcome::Slashed(err.clone()),
                        sequencer_da_address,
                        &mut checkpoint,
                    );

                    return (
                        Err(ApplyBatchError::Slashed {
                            hash: batch.id,
                            reason: err,
                            tx_receipts,
                            gas_price: gas_price.to_vec(),
                        }),
                        checkpoint,
                        gas_used,
                    );
                }
                Err(AuthenticationError::Invalid {
                    reason,
                    penalization_amount,
                }) => {
                    info!(
                        sequencer_da_address = %sequencer_da_address,
                        penalization_amount = %penalization_amount,
                        "Sequencer was penalized for the reason: {:?}",
                        reason
                    );

                    // Applies the outcome of the transaction execution to update the sequencer's state.
                    self.runtime.penalize_sequencer(
                        sequencer_da_address,
                        penalization_amount,
                        &mut checkpoint,
                    );
                }
                Ok((tx, message)) => {
                    // If the transaction is valid, execute it and apply the changes to the state.
                    let res = apply_tx(
                        &self.runtime,
                        &tx,
                        message,
                        checkpoint,
                        sequencer_da_address,
                        ExecutionMode::Normal,
                        gas_price,
                        height,
                    );

                    gas_used.combine(&S::Gas::from_slice(&res.receipt.gas_used));
                    tx_receipts.push(res.receipt);
                    accumulated_reward += res.sequencer_reward;
                    checkpoint = res.new_checkpoint;
                }
            }
        }

        let sequencer_outcome = SequencerOutcome::Rewarded(accumulated_reward);
        self.runtime
            .end_batch_hook(sequencer_outcome.clone(), sender, &mut checkpoint);

        (
            Ok(BatchReceipt {
                batch_hash: batch.id,
                tx_receipts,
                inner: sequencer_outcome,
                gas_price: gas_price.to_vec(),
            }),
            checkpoint,
            gas_used,
        )
    }
}

fn compute_sequencer_tx_reward<S: Spec>(
    tx: &AuthenticatedTransactionData<S>,
    gas_meter: &GasMeter<S::Gas>,
) -> u64 {
    // We transfer the consumed base fee to the base fee recipient address.
    let base_fee = gas_meter.gas_used().value(gas_meter.gas_price());
    // We compute the `max_priority_fee_bips` by applying the `max_priority_fee_bips` to the consumed gas.
    let max_priority_fee_bips = tx
        .max_priority_fee_bips()
        .apply(base_fee)
        // if the computation overflows, we return the max fee - we always have `priority_fee <= max_priority_fee_bips <= tx.max_fee()`
        .unwrap_or(tx.max_fee());

    // The tip is the minimum of the remaining gas allocated to the transaction and the maximum priority fee per gas.
    min(max_priority_fee_bips, tx.max_fee() - base_fee)
}

/// The result of applying a transaction to the state.
/// This is the return value of the [`apply_tx`] function.
/// It contains the new transaction checkpoint, transaction receipt and the amount of gas tokens that the sequencer should be rewarded.
///
/// # Note
/// If the sequencer is penalized within [`apply_tx`], the amount of gas tokens that the sequencer should be rewarded is set to 0.
pub struct ApplyTxResult<S: Spec> {
    /// The new state checkpoint after the transaction has been applied.
    pub new_checkpoint: StateCheckpoint<S>,
    /// The transaction receipt.
    pub receipt: TransactionReceipt<TxEffect>,
    /// The amount of gas tokens that the sequencer should be rewarded.
    pub sequencer_reward: u64,
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
    execution_mode: ExecutionMode,
    gas_price: &<S::Gas as Gas>::Price,
    height: u64,
) -> ApplyTxResult<S>
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
            error = %e,
            raw_tx_hash = hex::encode(raw_tx_hash),
            "Tx was rejected by the 'check_uniqueness' hook",
        );
        if execution_mode != ExecutionMode::Speculative {
            // We penalize the sequencer for the fixed amount of gas that was used to execute the transaction.
            runtime.penalize_sequencer(
                sequencer,
                tx.gas_fixed_cost().value(gas_price),
                &mut state_checkpoint,
            );
        }

        return ApplyTxResult {
            new_checkpoint: state_checkpoint,
            receipt: TransactionReceipt {
                tx_hash: *raw_tx_hash,
                body_to_save: None,
                events: vec![],
                receipt: TxEffect::Duplicate,
                gas_used: tx.gas_fixed_cost().to_vec(),
            },
            sequencer_reward: 0,
        };
    }

    let mut working_set = match runtime.try_reserve_gas(tx, &ctx, gas_price, state_checkpoint) {
        Ok(working_set) => working_set,
        Err(mut checkpoint) => {
            error!(
                raw_tx_hash = hex::encode(raw_tx_hash),
                "Tx was rejected by the 'try_reserve_gas' hook.",
            );
            if execution_mode != ExecutionMode::Speculative {
                // We penalize the sequencer for the fixed amount of gas that was used to execute the transaction.
                runtime.penalize_sequencer(
                    sequencer,
                    tx.gas_fixed_cost().value(gas_price),
                    &mut checkpoint,
                );
            }

            return ApplyTxResult {
                new_checkpoint: checkpoint,
                receipt: TransactionReceipt {
                    tx_hash: *raw_tx_hash,
                    body_to_save: None,
                    events: vec![],
                    receipt: TxEffect::InsufficientBaseGas,
                    gas_used: tx.gas_fixed_cost().to_vec(),
                },
                sequencer_reward: 0,
            };
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
                error = %e,
                raw_tx_hash = hex::encode(raw_tx_hash),
                "Tx was reverted",
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
            raw_tx_hash = hex::encode(raw_tx_hash),
            "Tx was unsuccessful: {:?}. Undoing all effects.",
            receipt.receipt
        );
        gas_meter.set_gas_funds(initial_funds);
        runtime.refund_remaining_gas(tx, &ctx, &gas_meter, &mut checkpoint);
        return ApplyTxResult {
            new_checkpoint: checkpoint,
            receipt,
            sequencer_reward: 0,
        };
    }

    runtime.mark_tx_attempted(tx, sequencer, &mut checkpoint);

    runtime.refund_remaining_gas(tx, &ctx, &gas_meter, &mut checkpoint);

    let gas_reward = compute_sequencer_tx_reward(tx, &gas_meter);

    debug!(
        tx_hash =
        hex::encode(raw_tx_hash),
        receipt= ?receipt.receipt,
        %gas_reward,
    "Sequencer reward has been updated after tx execution",
        );

    ApplyTxResult {
        new_checkpoint: checkpoint,
        receipt,
        sequencer_reward: gas_reward,
    }
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
