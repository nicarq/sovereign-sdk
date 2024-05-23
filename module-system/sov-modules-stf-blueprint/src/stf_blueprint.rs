use std::marker::PhantomData;

#[cfg(feature = "native")]
use borsh::BorshSerialize;
#[cfg(all(target_os = "zkvm", feature = "bench"))]
use risc0_cycle_macros::cycle_tracker;
use sov_modules_api::batch::BatchWithId;
use sov_modules_api::capabilities::{
    AuthenticationError, AuthorizeSequencerError, FatalError, GasEnforcer, HasCapabilities, RawTx,
    RuntimeAuthenticator, RuntimeAuthorization, SequencerAuthorization, TryReserveGasError,
};
use sov_modules_api::runtime::capabilities::KernelSlotHooks;
use sov_modules_api::transaction::{
    AuthenticatedTransactionAndRawHash, AuthenticatedTransactionData,
};
use sov_modules_api::{
    Context, DaSpec, DispatchCall, Gas, GasArray, PreExecWorkingSet, SequencerReward, Spec,
    StateCheckpoint, TxScratchpad, WorkingSet,
};
use sov_rollup_interface::stf::{BatchReceipt, StoredEvent, TransactionReceipt};
use tracing::{debug, error, warn};

use crate::{
    ApplyTxResult, BatchSequencerOutcome, Runtime, SkippedReason, TxEffect, TxProcessingError,
    TxProcessingErrorReason,
};

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
        Self {
            runtime: RT::default(),
            kernel: K::default(),
            phantom_context: PhantomData,
            phantom_da: PhantomData,
        }
    }
}

impl<S, Da, RT, K> StfBlueprint<S, Da, RT, K>
where
    S: Spec,
    Da: DaSpec,
    RT: Runtime<S, Da>,
    K: KernelSlotHooks<S, Da>,
{
    /// [`StfBlueprint`] constructor with the default [`Runtime`] value. Same as
    /// [`Default::default`].
    pub fn new() -> Self {
        Self::default()
    }

    /// [`StfBlueprint`] constructor with a custom [`Runtime`] value.
    pub fn with_runtime(runtime: RT) -> Self {
        Self {
            runtime,
            ..Default::default()
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
        sequencer_da_address: &Da::Address,
        gas_price: &<S::Gas as Gas>::Price,
        height: u64,
    ) -> (ApplyBatch, StateCheckpoint<S>, S::Gas) {
        debug!(
            batch_id = hex::encode(batch.id),
            sequencer_da_address = %sequencer_da_address,
            ?gas_price,
            "Applying a batch"
        );

        // ApplyBlobHook: begin
        if let Err(e) =
            self.runtime
                .begin_batch_hook(&mut batch, sequencer_da_address, &mut checkpoint)
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
            let tx_scratchpad = checkpoint.to_tx_scratchpad();
            let process_tx_result = process_tx(
                &self.runtime,
                raw_tx,
                sequencer_da_address,
                gas_price,
                height,
                tx_scratchpad,
            );

            match process_tx_result {
                Err(TxProcessingError {
                    tx_scratchpad,
                    reason,
                }) => {
                    checkpoint = tx_scratchpad.commit();
                    match reason {
                        TxProcessingErrorReason::SequencerUnauthorized(reason) => {
                            error!(
                                reason = %reason,
                                sequencer_da_address = %sequencer_da_address,
                                "Error: The transaction was rejected by the 'authorize_sequencer' capability. Dropping the remaining transactions in that batch",
                            );
                            break;
                        }

                        // If the sequencer raised a fatal error then he needs to get slashed and we stop applying the batch
                        TxProcessingErrorReason::AuthenticationError(
                            AuthenticationError::FatalError(err),
                        ) => {
                            error!(
                                sequencer_da_address = %sequencer_da_address,
                                err=%err, "Tx authentication raised a fatal error, sequencer slashed");

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

                        // In these cases the sequencer is penalized and we can just ignore the outcome
                        err => {
                            if let Ok((reason, raw_tx_hash)) =
                                TryInto::<(SkippedReason, [u8; 32])>::try_into(err)
                            {
                                warn!(
                                    error = %reason,
                                    raw_tx_hash = hex::encode(raw_tx_hash),
                                    "An error occurred while processing a transaction. The transaction was not executed. The sequencer was penalized.",
                                );

                                let tx_receipt = TransactionReceipt {
                                    tx_hash: raw_tx_hash,
                                    body_to_save: None,
                                    events: Vec::new(),
                                    receipt: TxEffect::Skipped(reason),
                                    gas_used: S::Gas::zero().to_vec(),
                                };

                                tx_receipts.push(tx_receipt);
                            }
                        }
                    }
                }
                Ok(ApplyTxResult {
                    tx_scratchpad,
                    receipt,
                    sequencer_reward,
                }) => {
                    checkpoint = tx_scratchpad.commit();

                    gas_used.combine(&S::Gas::from_slice(&receipt.gas_used));
                    tx_receipts.push(receipt);

                    accumulated_reward.accumulate(sequencer_reward);
                }
            }
        }

        let sequencer_outcome = BatchSequencerOutcome::Rewarded(accumulated_reward);
        self.runtime.end_batch_hook(
            sequencer_outcome.clone(),
            sequencer_da_address,
            &mut checkpoint,
        );

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

/// Executes the entire transaction lifecycle.
#[allow(clippy::result_large_err)]
pub fn process_tx<S: Spec, D: DaSpec, R: Runtime<S, D>>(
    runtime: &R,
    raw_tx: &RawTx,
    // TODO <`https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/728`>: group constant variables in the stf-blueprint
    sequencer_da_address: &D::Address,
    gas_price: &<S::Gas as Gas>::Price,
    height: u64,
    scratchpad: TxScratchpad<S>,
) -> Result<ApplyTxResult<S>, TxProcessingError<S>> {
    // Checks the sequencer balance before the transaction is executed.
    // If the sequencer balance is not high enough, the transaction is rejected.
    let mut pre_exec_working_set = match runtime.capabilities().authorize_sequencer(
        sequencer_da_address,
        gas_price,
        scratchpad,
    ) {
        Ok(pre_exec_working_set) => pre_exec_working_set,
        Err(AuthorizeSequencerError {
            reason,
            tx_scratchpad,
        }) => {
            return Err(TxProcessingError {
                tx_scratchpad,
                reason: TxProcessingErrorReason::SequencerUnauthorized(reason.to_string()),
            });
        }
    };

    let (tx, message) =
        match authenticate_with_cycle_count(runtime, raw_tx, &mut pre_exec_working_set) {
            Err(AuthenticationError::FatalError(reason)) => {
                return Err(TxProcessingError {
                    tx_scratchpad: pre_exec_working_set.into(),
                    reason: TxProcessingErrorReason::AuthenticationError(
                        AuthenticationError::FatalError(reason),
                    ),
                });
            }
            Err(AuthenticationError::Invalid(reason)) => {
                // Applies the outcome of the transaction execution to update the sequencer's state.
                let tx_scratchpad = runtime
                    .capabilities()
                    .penalize_sequencer(sequencer_da_address, pre_exec_working_set);

                return Err(TxProcessingError {
                    tx_scratchpad,
                    reason: TxProcessingErrorReason::AuthenticationError(
                        AuthenticationError::Invalid(reason),
                    ),
                });
            }
            Ok((tx, message)) => (tx, message),
        };

    let raw_tx_hash = &tx.raw_tx_hash;
    let tx = &tx.authenticated_tx;

    let maybe_ctx = runtime.capabilities().resolve_context(
        tx,
        sequencer_da_address,
        height,
        &mut pre_exec_working_set,
    );
    let ctx = match maybe_ctx {
        Ok(ctx) => ctx,
        Err(e) => {
            // We penalize the sequencer for the fixed amount of gas that was used to execute the transaction.
            let tx_scratchpad = runtime
                .capabilities()
                .penalize_sequencer(sequencer_da_address, pre_exec_working_set);

            return Err(TxProcessingError {
                tx_scratchpad,
                reason: TxProcessingErrorReason::CannotResolveContext {
                    reason: e.to_string(),
                    raw_tx_hash: *raw_tx_hash,
                },
            });
        }
    };

    // Check that the transaction isn't a duplicate
    if let Err(e) = runtime
        .capabilities()
        .check_uniqueness(tx, &ctx, &mut pre_exec_working_set)
    {
        // We penalize the sequencer for the fixed amount of gas that was used to execute the transaction.
        let tx_scratchpad = runtime
            .capabilities()
            .penalize_sequencer(sequencer_da_address, pre_exec_working_set);

        return Err(TxProcessingError {
            tx_scratchpad,
            reason: TxProcessingErrorReason::Nonce {
                reason: e.to_string(),
                raw_tx_hash: *raw_tx_hash,
            },
        });
    }

    let working_set = match runtime
        .capabilities()
        .try_reserve_gas(tx, &ctx, pre_exec_working_set)
    {
        Ok(working_set) => working_set,
        Err(TryReserveGasError {
            reason,
            pre_exec_working_set,
        }) => {
            // We penalize the sequencer for the fixed amount of gas that was used to execute the transaction.
            let tx_scratchpad = runtime
                .capabilities()
                .penalize_sequencer(sequencer_da_address, pre_exec_working_set);

            return Err(TxProcessingError {
                tx_scratchpad,
                reason: TxProcessingErrorReason::CannotReserveGas {
                    reason: reason.to_string(),
                    raw_tx_hash: *raw_tx_hash,
                },
            });
        }
    };

    // If the transaction is valid, execute it and apply the changes to the state.
    Ok(apply_tx(
        runtime,
        ctx,
        tx,
        raw_tx_hash,
        message,
        working_set,
        sequencer_da_address,
    ))
}

#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
fn authenticate_with_cycle_count<S: Spec, Da: DaSpec, R: Runtime<S, Da>>(
    runtime: &R,
    raw_tx: &RawTx,
    pre_exec_working_set: &mut PreExecWorkingSet<
        S,
        <R as HasCapabilities<S, Da>>::SequencerStakeMeter,
    >,
) -> Result<
    (
        AuthenticatedTransactionAndRawHash<S>,
        <R as RuntimeAuthenticator<S>>::Decodable,
    ),
    AuthenticationError,
> {
    runtime.authenticate(raw_tx, pre_exec_working_set)
}

/// Applies a single transaction to the current state. In normal execution, we commit twice times execution:
/// 1. After the pre-dispatch hook. This ensures that the gas charges are paid even if the transaction fails later during execution
/// 2. After the post-dispatch hook. This ensures that the transaction can be reverted by the post-dispatch hook if desired.
#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
#[allow(clippy::too_many_arguments)]
fn apply_tx<S, RT, Da>(
    runtime: &RT,
    ctx: Context<S>,
    tx: &AuthenticatedTransactionData<S>,
    raw_tx_hash: &[u8; 32],
    message: <RT as DispatchCall>::Decodable,
    mut working_set: WorkingSet<S>,
    sequencer: &Da::Address,
) -> ApplyTxResult<S>
where
    S: Spec,
    Da: DaSpec,
    RT: Runtime<S, Da>,
{
    let tx_result = attempt_tx(tx, message, &ctx, runtime, &mut working_set);
    let (mut tx_scratchpad, receipt, transaction_consumption) = match tx_result {
        Ok(_) => {
            let (tx_scratchpad, transaction_consumption, events) = working_set.finalize();

            (
                tx_scratchpad,
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
            let (tx_scratchpad, transaction_consumption) = working_set.revert();

            let receipt = TransactionReceipt {
                tx_hash: *raw_tx_hash,
                body_to_save: None,
                events: vec![], // As in Ethereum, reverted transactions don't emit events
                receipt: TxEffect::Reverted,
                gas_used: transaction_consumption.base_fee().to_vec(),
            };

            (tx_scratchpad, receipt, transaction_consumption)
        }
    };

    runtime
        .capabilities()
        .mark_tx_attempted(tx, sequencer, &mut tx_scratchpad);

    runtime
        .capabilities()
        .refund_remaining_gas(&ctx, &transaction_consumption, &mut tx_scratchpad);

    runtime
        .capabilities()
        .allocate_consumed_gas(&transaction_consumption, &mut tx_scratchpad);

    debug!(
        tx_hash =
        hex::encode(raw_tx_hash),
        receipt= ?receipt.receipt,
        consumption= %transaction_consumption,
    "Transaction has been successfully executed",
        );

    ApplyTxResult::<S> {
        tx_scratchpad,
        receipt,
        sequencer_reward: transaction_consumption.into(),
    }
}

fn attempt_tx<S: Spec, Da: DaSpec, RT: Runtime<S, Da>>(
    tx: &AuthenticatedTransactionData<S>,
    message: <RT as DispatchCall>::Decodable,
    ctx: &Context<S>,
    runtime: &RT,
    working_set: &mut WorkingSet<S>,
) -> Result<(), anyhow::Error> {
    runtime.pre_dispatch_tx_hook(tx, working_set)?;

    runtime.dispatch_call(message, working_set, ctx)?;

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
