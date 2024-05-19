use std::marker::PhantomData;

#[cfg(feature = "native")]
use borsh::BorshSerialize;
#[cfg(all(target_os = "zkvm", feature = "bench"))]
use risc0_cycle_macros::cycle_tracker;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::capabilities::{
    AuthenticationError, FatalError, RawTx, RuntimeAuthenticator, SequencerAuthorization,
};
use sov_modules_api::runtime::capabilities::KernelSlotHooks;
use sov_modules_api::transaction::{
    AuthenticatedTransactionAndRawHash, AuthenticatedTransactionData,
};
use sov_modules_api::{
    Context, DaSpec, DispatchCall, Gas, GasArray, GasMeter, SequencerReward, Spec, StateCheckpoint,
    TransactionConsumption, WorkingSet,
};
use sov_rollup_interface::stf::{BatchReceipt, StoredEvent, TransactionReceipt};
use tracing::{debug, error, info};

use crate::{BatchSequencerOutcome, Runtime, TxEffect, TxSequencerOutcome};

type ApplyBatchResult<T> = Result<T, ApplyBatchError<TxEffect>>;

#[allow(type_alias_bounds)]
pub(crate) type ApplyBatch = ApplyBatchResult<BatchReceipt<BatchSequencerOutcome, TxEffect>>;

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

impl From<ApplyBatchError<TxEffect>> for BatchReceipt<BatchSequencerOutcome, TxEffect> {
    fn from(value: ApplyBatchError<TxEffect>) -> Self {
        match value {
            ApplyBatchError::Ignored(hash) => BatchReceipt {
                batch_hash: hash,
                tx_receipts: Vec::new(),
                inner: BatchSequencerOutcome::Ignored,
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
                inner: BatchSequencerOutcome::Slashed(reason),
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
        let mut accumulated_reward = SequencerReward::ZERO;

        debug!(
            batch_id = hex::encode(batch.id),
            txs_num = raw_txs.len(),
            "Verifying & executing transactions"
        );

        for raw_tx in raw_txs.iter() {
            let sequencer_da_address = sender;
            // Checks the sequencer balance before the transaction is executed.
            // If the sequencer balance is not high enough, the transaction is rejected.
            let mut sequencer_stake_meter = match self.runtime.authorize_sequencer(
                sequencer_da_address,
                gas_price,
                &mut checkpoint,
            ) {
                Ok(sequencer_stake_meter) => sequencer_stake_meter,
                Err(e) => {
                    error!(
                        error = %e,
                        batch_id = hex::encode(batch.id),
                        sequencer_da_address = %sequencer_da_address,
                        "Error: The batch was rejected by the 'authorize_sequencer' capability. Skipping batch without slashing the sequencer",
                    );

                    break;
                }
            };

            match authenticate_with_cycle_count(&self.runtime, raw_tx, &mut sequencer_stake_meter) {
                Err(AuthenticationError::FatalError(err)) => {
                    error!(err=%err, "Tx authentication failed");

                    self.runtime.end_batch_hook(
                        BatchSequencerOutcome::Slashed(err.clone()),
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
                Err(AuthenticationError::Invalid(reason)) => {
                    info!(
                        sequencer_da_address = %sequencer_da_address,
                        penalization_amount = %sequencer_stake_meter.gas_used_value(),
                        "Sequencer was penalized during transaction authentication for the reason: {:?}",
                        reason
                    );

                    // Applies the outcome of the transaction execution to update the sequencer's state.
                    self.runtime.penalize_sequencer(
                        sequencer_da_address,
                        sequencer_stake_meter,
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
                        sequencer_stake_meter,
                        ExecutionMode::Normal,
                        gas_price,
                        height,
                    );

                    gas_used.combine(&S::Gas::from_slice(&res.receipt.gas_used));
                    tx_receipts.push(res.receipt);

                    if let TxSequencerOutcome::Rewarded(sequencer_reward) = res.tx_sequencer_outcome
                    {
                        accumulated_reward.accumulate(sequencer_reward);
                    }

                    checkpoint = res.new_checkpoint;
                }
            }
        }

        let sequencer_outcome = BatchSequencerOutcome::Rewarded(accumulated_reward);
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

#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
fn authenticate_with_cycle_count<S: Spec, D: DaSpec, R: Runtime<S, D> + RuntimeAuthenticator<S>>(
    runtime: &R,
    raw_tx: &RawTx,
    sequencer_stake_meter: &mut <R as RuntimeAuthenticator<S>>::SequencerStakeMeter,
) -> Result<
    (
        AuthenticatedTransactionAndRawHash<S>,
        <R as RuntimeAuthenticator<S>>::Decodable,
    ),
    AuthenticationError,
> {
    runtime.authenticate(raw_tx, sequencer_stake_meter)
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
    pub tx_sequencer_outcome: TxSequencerOutcome,
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
    sequencer_stake_meter: <RT as SequencerAuthorization<S, Da>>::SequencerStakeMeter,
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

    let maybe_ctx = runtime.resolve_context(tx, sequencer, height, &mut state_checkpoint);
    let ctx = match maybe_ctx {
        Ok(ctx) => ctx,
        Err(e) => {
            error!(
                error = %e,
                raw_tx_hash = hex::encode(raw_tx_hash),
                sequencer_penalization_amount = %sequencer_stake_meter.gas_used_value(),
                "Tx was rejected by the 'resolve_context' hook",
            );

            if execution_mode != ExecutionMode::Speculative {
                // We penalize the sequencer for the fixed amount of gas that was used to execute the transaction.
                runtime.penalize_sequencer(sequencer, sequencer_stake_meter, &mut state_checkpoint);
            }

            return ApplyTxResult {
                new_checkpoint: state_checkpoint,
                receipt: TransactionReceipt {
                    tx_hash: *raw_tx_hash,
                    body_to_save: None,
                    events: vec![],
                    receipt: TxEffect::CannotResolveContext,
                    gas_used: <S::Gas as Gas>::zero().to_vec(),
                },
                tx_sequencer_outcome: TxSequencerOutcome::Penalized,
            };
        }
    };

    // Check that the transaction isn't a duplicate
    if let Err(e) = runtime.check_uniqueness(tx, &ctx, &mut state_checkpoint) {
        error!(
            error = %e,
            raw_tx_hash = hex::encode(raw_tx_hash),
            sequencer_penalization_amount = %sequencer_stake_meter.gas_used_value(),
            "Tx was rejected by the 'check_uniqueness' hook",
        );

        if execution_mode != ExecutionMode::Speculative {
            // We penalize the sequencer for the fixed amount of gas that was used to execute the transaction.
            runtime.penalize_sequencer(sequencer, sequencer_stake_meter, &mut state_checkpoint);
        }

        return ApplyTxResult {
            new_checkpoint: state_checkpoint,
            receipt: TransactionReceipt {
                tx_hash: *raw_tx_hash,
                body_to_save: None,
                events: vec![],
                receipt: TxEffect::Duplicate,
                // The gas used is always zero here, since we didn't reserve any gas for the transaction yet.
                gas_used: <S::Gas as Gas>::zero().to_vec(),
            },
            tx_sequencer_outcome: TxSequencerOutcome::Penalized,
        };
    }

    let mut working_set = match runtime.try_reserve_gas(
        tx,
        &ctx,
        gas_price,
        &sequencer_stake_meter,
        state_checkpoint,
    ) {
        Ok(working_set) => working_set,
        Err(mut checkpoint) => {
            error!(
                raw_tx_hash = hex::encode(raw_tx_hash),
                sequencer_penalization_amount = %sequencer_stake_meter.gas_used_value(),
                "Tx was rejected by the 'try_reserve_gas' hook. The gas reserve check failed.",
            );

            if execution_mode != ExecutionMode::Speculative {
                // We penalize the sequencer for the fixed amount of gas that was used to execute the transaction.
                runtime.penalize_sequencer(sequencer, sequencer_stake_meter, &mut checkpoint);
            }

            return ApplyTxResult {
                new_checkpoint: checkpoint,
                receipt: TransactionReceipt {
                    tx_hash: *raw_tx_hash,
                    body_to_save: None,
                    events: vec![],
                    receipt: TxEffect::CannotReserveGas,
                    gas_used: <S::Gas as Gas>::zero().to_vec(),
                },
                tx_sequencer_outcome: TxSequencerOutcome::Penalized,
            };
        }
    };

    let tx_result = attempt_tx(runtime, tx, message, &mut working_set, &ctx);
    let (mut checkpoint, receipt, transaction_consumption) = match tx_result {
        Ok(_) => {
            let (checkpoint, transaction_consumption, events) = working_set.checkpoint();

            (
                checkpoint,
                TransactionReceipt {
                    tx_hash: *raw_tx_hash,
                    body_to_save: None,
                    events: convert_to_runtime_events::<S, RT, Da>(events),
                    receipt: TxEffect::Successful,
                    gas_used: transaction_consumption.base_fee().to_vec(),
                },
                transaction_consumption,
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
            let (mut checkpoint, transaction_consumption) = working_set.revert();

            let receipt = TransactionReceipt {
                tx_hash: *raw_tx_hash,
                body_to_save: None,
                events: vec![], // As in Ethereum, reverted transactions don't emit events
                receipt: TxEffect::Reverted,
                gas_used: transaction_consumption.base_fee().to_vec(),
            };

            // If the transaction failed in speculative mode, we act as though it never happened.
            if execution_mode == ExecutionMode::Speculative {
                tracing::info!(
                    raw_tx_hash = hex::encode(raw_tx_hash),
                    "Tx was unsuccessful: {:?}. Undoing all effects.",
                    receipt.receipt
                );
                runtime.refund_remaining_gas(
                    tx,
                    &ctx,
                    &TransactionConsumption::ZERO,
                    &mut checkpoint,
                );
                return ApplyTxResult {
                    new_checkpoint: checkpoint,
                    receipt,
                    tx_sequencer_outcome: TxSequencerOutcome::Ignored,
                };
            }

            (checkpoint, receipt, transaction_consumption)
        }
    };

    runtime.mark_tx_attempted(tx, sequencer, &mut checkpoint);

    runtime.allocate_consumed_gas(&transaction_consumption, &mut checkpoint);
    runtime.refund_remaining_gas(tx, &ctx, &transaction_consumption, &mut checkpoint);

    debug!(
        tx_hash =
        hex::encode(raw_tx_hash),
        receipt= ?receipt.receipt,
        consumption= %transaction_consumption,
    "Transaction has been successfully executed",
        );

    ApplyTxResult {
        new_checkpoint: checkpoint,
        receipt,
        tx_sequencer_outcome: TxSequencerOutcome::Rewarded(transaction_consumption.into()),
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
    runtime.pre_dispatch_tx_hook(tx, working_set)?;

    runtime
        .dispatch_call(message, working_set, ctx)
        .map_err(Into::<anyhow::Error>::into)?;

    runtime.post_dispatch_tx_hook(tx, ctx, working_set)?;

    Ok(())
}

#[cfg(feature = "native")]
pub(crate) fn convert_to_runtime_events<S, RT, Da>(
    events: Vec<sov_modules_api::TypedEvent>,
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
    _events: Vec<sov_modules_api::TypedEvent>,
) -> Vec<StoredEvent>
where
    S: Spec,
    Da: DaSpec,
    RT: Runtime<S, Da>,
{
    Vec::new() // Return an empty vector
}
