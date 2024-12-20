use borsh::BorshDeserialize;
#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_cycle_utils::macros::cycle_tracker;
use sov_modules_api::capabilities::{
    fatal_deserialization_error, AuthenticationError, AuthorizeSequencerError, GasEnforcer,
    SequencerAuthorization, SequencerRemuneration, TransactionAuthorizer, TryReserveGasError,
};
use sov_modules_api::transaction::TransactionConsumption;
#[cfg(feature = "native")]
use sov_modules_api::NestedEnumUtils;
use sov_modules_api::{
    BasicGasMeter, BatchSequencerOutcome, BatchSequencerReceipt, DaSpec, ExecutionContext,
    FullyBakedTx, Gas, GasArray, GasMeter, IncrementalBatch, InjectedControlFlow,
    PreExecWorkingSet, ProvisionalSequencerOutcome, Rewards, Spec, StateCheckpoint, StateProvider,
    TxControlFlow, TxScratchpad, WorkingSet,
};
use sov_rollup_interface::TxHash;
use tracing::{debug, warn};

use super::common::ValidatedAuthOutput;
pub use crate::sequencer_mode::common::PreExecError;
use crate::sequencer_mode::common::{
    apply_batch_logs, apply_tx, create_tx_receipt, get_gas_used, BatchReceipt,
};
use crate::{
    ApplyTxResult, AuthTxOutput, Runtime, SkippedTxContents, TransactionReceipt, TxProcessingError,
};

/// Executes the entire transaction lifecycle.
#[allow(clippy::result_large_err, clippy::too_many_arguments)]
#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
pub fn process_tx<S, R, I, C>(
    runtime: &R,
    validated_output: ValidatedAuthOutput<S, R>,
    gas_price: &<S::Gas as Gas>::Price,
    gas_used_for_authentication: &S::Gas,
    sequencer_da_address: &<S::Da as DaSpec>::Address,
    height: u64,
    scratchpad: TxScratchpad<S, I>,
    execution_context: ExecutionContext,
    injected_control_flow: &C,
) -> (
    Result<ApplyTxResult<S>, TxProcessingError>,
    TxScratchpad<S, I>,
)
where
    S: Spec,
    R: Runtime<S>,
    I: StateProvider<S>,
    C: InjectedControlFlow<TransactionReceipt<S>, S>,
{
    #[cfg(feature = "native")]
    let (start, discriminant) = {
        let start = std::time::Instant::now();
        let discriminant = match &validated_output {
            ValidatedAuthOutput::Valid((_, _, message)) => format!("{:?}", message.discriminant()),
            ValidatedAuthOutput::Invalid { .. } => "Unknown".to_string(),
        };
        (start, discriminant)
    };

    let result = process_tx_inner(
        runtime,
        validated_output,
        gas_price,
        gas_used_for_authentication,
        sequencer_da_address,
        height,
        scratchpad,
        execution_context,
        injected_control_flow,
    );

    #[cfg(feature = "native")]
    track_transaction_metrics(
        &result,
        start.elapsed(),
        execution_context,
        height,
        sequencer_da_address,
        discriminant,
    );

    result
}

#[cfg(feature = "native")]
fn track_transaction_metrics<S: Spec>(
    result: &(
        Result<ApplyTxResult<S>, TxProcessingError>,
        TxScratchpad<S, impl StateProvider<S>>,
    ),
    execution_time: std::time::Duration,
    execution_context: ExecutionContext,
    height: u64,
    sequencer_address: &<S::Da as DaSpec>::Address,
    message_discriminant: String,
) {
    sov_metrics::track_metrics(|metrics_tracker| {
        let tx_effect = match &result.0 {
            Ok(tx_result) => sov_metrics::TransactionEffect::from(&tx_result.receipt.receipt),
            Err(_) => sov_metrics::TransactionEffect::Skipped,
        };

        let transaction_metrics = sov_metrics::TransactionProcessingMetrics {
            execution_time,
            tx_effect,
            execution_context,
            rollup_height: height,
            sequencer_address: sequencer_address.to_string(),
            call_message: message_discriminant,
        };

        metrics_tracker.track_transaction_processing(transaction_metrics);
    });
}

/// Actual processing of transaction.
#[allow(clippy::result_large_err, clippy::too_many_arguments)]
fn process_tx_inner<S, R, I, C>(
    runtime: &R,
    validated_output: ValidatedAuthOutput<S, R>,
    gas_price: &<S::Gas as Gas>::Price,
    gas_used_for_authentication: &S::Gas,
    sequencer_da_address: &<S::Da as DaSpec>::Address,
    height: u64,
    mut scratchpad: TxScratchpad<S, I>,
    execution_context: ExecutionContext,
    injected_control_flow: &C,
) -> (
    Result<ApplyTxResult<S>, TxProcessingError>,
    TxScratchpad<S, I>,
)
where
    S: Spec,
    R: Runtime<S>,
    I: StateProvider<S>,
    C: InjectedControlFlow<TransactionReceipt<S>, S>,
{
    let auth_cost = gas_used_for_authentication.value(gas_price);

    let penalize = |tx_scratchpad: &mut TxScratchpad<S, I>| {
        runtime
            .gas_enforcer()
            .transfer_funds_from_sequencer_to_prover(auth_cost, sequencer_da_address, tx_scratchpad)
            // We ensured this before entering the tx execution loop.
            .expect("Sequencer should have enough funds to pay for the pre-execution checks");
    };

    let (auth_tx, auth_data, message) = match validated_output {
        ValidatedAuthOutput::Valid(valid) => valid,
        ValidatedAuthOutput::Invalid { tx_hash, error } => {
            penalize(&mut scratchpad);

            return (
                Err(TxProcessingError::AuthenticationFailed(format!(
                    "Authentication failed for tx: {}. Error: {}",
                    tx_hash, error
                ))),
                scratchpad,
            );
        }
    };

    let raw_tx_hash = auth_tx.raw_tx_hash;
    let tx = &auth_tx.authenticated_tx;

    let maybe_ctx = runtime.transaction_authorizer().resolve_context(
        &auth_data,
        sequencer_da_address,
        height,
        &mut scratchpad,
        execution_context,
    );
    let mut ctx = match maybe_ctx {
        Ok(ctx) => ctx,
        Err(err) => {
            penalize(&mut scratchpad);

            return (
                Err(TxProcessingError::CannotResolveContext(err.to_string())),
                scratchpad,
            );
        }
    };

    match injected_control_flow.pre_flight(runtime, &ctx, &message) {
        TxControlFlow::ContinueProcessing(_) => {}
        TxControlFlow::IgnoreTx => {
            return (
                Err(TxProcessingError::RejectedByPreFlight),
                scratchpad.revert().to_tx_scratchpad(),
            )
        }
    }

    // Check that the transaction isn't a duplicate
    if let Err(err) =
        runtime
            .transaction_authorizer()
            .check_uniqueness(&auth_data, &ctx, &mut scratchpad)
    {
        penalize(&mut scratchpad);

        return (
            Err(TxProcessingError::IncorrectNonce(err.to_string())),
            scratchpad,
        );
    }

    if let Err(TryReserveGasError { reason }) =
        runtime
            .gas_enforcer()
            .try_reserve_gas(tx, gas_price, &mut ctx, &mut scratchpad)
    {
        penalize(&mut scratchpad);

        return (
            Err(TxProcessingError::CannotReserveGas(reason.to_string())),
            scratchpad,
        );
    }

    let mut working_set = WorkingSet::create_working_set(scratchpad, gas_price, tx);

    // Recover the authentication cost form the user.
    if let Err(err) = working_set.charge_gas(gas_used_for_authentication) {
        let (mut scratchpad, transaction_consumption) = working_set.revert();
        penalize(&mut scratchpad);

        // Refund the remaining gas to the sender.
        runtime.gas_enforcer().refund_remaining_gas(
            ctx.gas_refund_recipient(),
            &transaction_consumption.remaining_funds(),
            &mut scratchpad,
        );

        return (
            Err(TxProcessingError::OutOfGas(err.to_string())),
            scratchpad,
        );
    }

    // If the transaction is valid, execute it and apply the changes to the state.
    let (apply_tx, mut scratchpad) = apply_tx(runtime, &ctx, tx, raw_tx_hash, message, working_set);

    let transaction_consumption = &apply_tx.transaction_consumption;

    runtime.transaction_authorizer().mark_tx_attempted(
        &auth_data,
        sequencer_da_address,
        &mut scratchpad,
    );

    runtime.gas_enforcer().refund_remaining_gas(
        ctx.gas_refund_recipient(),
        &transaction_consumption.remaining_funds(),
        &mut scratchpad,
    );

    runtime
        .gas_enforcer()
        .reward_prover(&transaction_consumption.base_fee_value(), &mut scratchpad);

    let sequencer_reward = transaction_consumption.priority_fee();
    runtime.sequencer_remuneration().reward_sequencer(
        sequencer_da_address,
        sequencer_reward,
        &mut scratchpad,
    );

    (Ok(apply_tx), scratchpad)
}

#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
fn deserialize_and_authenticate<S: Spec, R: Runtime<S>, I: StateProvider<S>>(
    runtime: &R,
    tx: &FullyBakedTx,
    pre_exec_working_set: &mut PreExecWorkingSet<S, I>,
) -> Result<AuthTxOutput<S, R>, AuthenticationError> {
    let auth_input = deserialize_tx(tx).map_err(|e| {
        fatal_deserialization_error::<PreExecWorkingSet<S, I>, S, _>(
            &tx.data,
            e,
            pre_exec_working_set,
        )
    })?;
    runtime.authenticate(&auth_input, pre_exec_working_set)
}

#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
fn deserialize_tx<T: BorshDeserialize>(tx: &FullyBakedTx) -> std::io::Result<T> {
    borsh::from_slice(&tx.data)
}

pub struct IncrementalBatchReceipt<S: Spec> {
    pub tx_receipts: Vec<TransactionReceipt<S>>,
    pub inner: BatchSequencerReceipt<S>,
}

impl<S: Spec> IncrementalBatchReceipt<S> {
    pub fn finalize(self, id: [u8; 32]) -> BatchReceipt<S> {
        BatchReceipt {
            batch_hash: id,
            tx_receipts: self.tx_receipts,
            inner: self.inner,
        }
    }
}

#[tracing::instrument(skip_all, name = "StfBlueprint::apply_batch")]
#[allow(clippy::too_many_arguments)]
#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
pub(crate) fn apply_batch<S, RT, B>(
    runtime: &RT,
    mut checkpoint: StateCheckpoint<S::Storage>,
    batch_with_id: B,
    blob_idx: usize,
    sequencer_da_address: <S::Da as DaSpec>::Address,
    gas_price: &<S::Gas as Gas>::Price,
    height: u64,
    execution_context: ExecutionContext,
) -> (IncrementalBatchReceipt<S>, StateCheckpoint<S::Storage>)
where
    S: Spec,
    RT: Runtime<S>,
    B: IncrementalBatch<crate::TransactionReceipt<S>, S>,
{
    let span = if let Some(id) = batch_with_id.id() {
        tracing::info_span!("batch", batch_id = hex::encode(id)).entered()
    } else {
        tracing::info_span!("sequencer-batch").entered()
    };
    debug!(
        sequencer_da_address = %sequencer_da_address,
        ?gas_price,
        "Applying a batch"
    );

    let mut clean_scratchpad = checkpoint.to_tx_scratchpad();

    let ignored_batch = |reason, seq_da_address, gas_used| IncrementalBatchReceipt {
        tx_receipts: Vec::new(),
        inner: BatchSequencerReceipt {
            da_address: seq_da_address,
            gas_price: gas_price.clone(),
            gas_used,
            outcome: BatchSequencerOutcome::Ignored(reason),
        },
    };

    debug!("Verifying & executing transactions");

    // Cost of the authentication for the entire batch.
    // It should include the costs of `authentication` and process_tx pre-execution checks.
    if execution_context == ExecutionContext::Node {
        assert!(
            batch_with_id.known_remaining_txs().is_some(),
            "Batch sizes are always known by the time the batch appears on the DA Layer"
        );
    }
    // SECURITY: We rely on the assumption that `known_remaining_txs` is *always* Some during
    // actual on-chain execution. `known_remaining_txs` may be `None` only during sequencing
    // Since this is the case, `conservative_max_sequencer_gas_costs` is a *true* upper bound on
    // costs during actual node execution. This is important, because if we run out of sequencer gas
    // we revert the *entire* batch. If this were to happen during real execution, this would
    // creates work for the prover without a corresponding gas payment, which could
    // become a DOS vector.
    let conservative_max_sequencer_gas_costs = match runtime
        .gas_enforcer()
        .max_tx_check_costs()
        .value(gas_price)
        // We multiply the min gas cost by the number of items in the batch, if known. If the number is unknwon
        // (because the batch is still being built) we require enough gas for at least 1 tx
        .checked_mul(batch_with_id.known_remaining_txs().unwrap_or(1) as u64)
    {
        Some(cost) => cost,
        None => {
            return (
                ignored_batch(
                    "The calculation of the maximum authentication cost resulted in an overflow"
                        .to_string(),
                    sequencer_da_address,
                    <S as Spec>::Gas::ZEROED,
                ),
                clean_scratchpad.revert(), // This revert is a no-op that just does type transformation
            );
        }
    };

    // Begin the transaction authorization phase.
    match runtime.sequencer_authorization().authorize_sequencer(
        &sequencer_da_address,
        conservative_max_sequencer_gas_costs,
        &mut clean_scratchpad,
    ) {
        Ok(_) => {}
        Err(AuthorizeSequencerError { reason }) => {
            let err_str = format!("Not enough gas to authenticate a transaction: {}", reason);

            warn!(
                error = %reason,
                "Not enough gas to authenticate the batch",
            );

            return (
                ignored_batch(err_str, sequencer_da_address, <S as Spec>::Gas::ZEROED),
                clean_scratchpad.commit(),
            );
        }
    };

    let mut tx_receipts = Vec::with_capacity(batch_with_id.known_remaining_txs().unwrap_or(128));
    let mut accumulated_reward = 0;
    let mut accumulated_penalty = 0;
    let mut total_gas_used = <S as Spec>::Gas::ZEROED;

    for (idx, (raw_tx, injected_control_flow)) in batch_with_id.enumerate() {
        let gas_meter = BasicGasMeter::new(
            runtime.gas_enforcer().max_tx_check_costs().value(gas_price),
            gas_price.clone(),
        );
        let AuthAndProcessOutput {
            gas_used,
            scratchpad: dirty_scratchpad,
            outcome,
        } = auth_and_process_tx(
            runtime,
            clean_scratchpad,
            &raw_tx,
            &sequencer_da_address,
            gas_price,
            height,
            execution_context,
            gas_meter,
            idx,
            &injected_control_flow,
        );
        let provisional_outcome = match outcome {
            AuthAndProcessOutcome::IllegalSequencer { reason } => {
                tracing::warn!("Transaction could not be attempted due to sequencer error. If this error persists, check that your sequencer has sufficient funds. Error: {}", reason);
                assert!(execution_context.is_sequencer(), "Attempted to run pre-execution checks without reserving sufficient gas. This is a bug! Please report it.");
                ProvisionalSequencerOutcome::out_of_funds(&gas_used, gas_price)
            }
            AuthAndProcessOutcome::Skipped { error, tx_hash } => {
                ProvisionalSequencerOutcome::penalize(
                    &gas_used,
                    gas_price,
                    create_tx_receipt(
                        SkippedTxContents {
                            error,
                            gas_used: gas_used.clone(),
                        },
                        tx_hash,
                    ),
                )
            }
            AuthAndProcessOutcome::Applied {
                transaction_consumption,
                receipt,
            } => ProvisionalSequencerOutcome::reward(
                transaction_consumption.priority_fee().0,
                receipt,
            ),
        };

        let provisional_reward = provisional_outcome.reward;
        let provisional_penalty = provisional_outcome.penalty;
        let (new_checkpoint, outcome) =
            injected_control_flow.post_tx(provisional_outcome, dirty_scratchpad);
        match outcome {
            TxControlFlow::ContinueProcessing(receipt) => {
                total_gas_used.combine(&gas_used);
                accumulated_reward += provisional_reward;
                accumulated_penalty += provisional_penalty;
                tx_receipts.push(receipt);
            }
            TxControlFlow::IgnoreTx => {}
        }
        clean_scratchpad = new_checkpoint.to_tx_scratchpad();
    }
    // End of the transaction processing phase.

    let batch_receipt = IncrementalBatchReceipt {
        tx_receipts,
        inner: BatchSequencerReceipt {
            da_address: sequencer_da_address,
            gas_price: gas_price.clone(),
            gas_used: total_gas_used.clone(),
            outcome: BatchSequencerOutcome::Executed(Rewards {
                accumulated_reward,
                accumulated_penalty,
            }),
        },
    };

    checkpoint = clean_scratchpad.commit();
    apply_batch_logs(&batch_receipt, &total_gas_used, blob_idx);
    span.exit();
    (batch_receipt, checkpoint)
}

enum AuthAndProcessOutcome<S: Spec> {
    /// The sequencer was not allowed to process this transaction
    IllegalSequencer { reason: String },
    /// The transaction failed before execution started
    Skipped {
        error: TxProcessingError,
        tx_hash: TxHash,
    },
    /// The transaction failed
    Applied {
        transaction_consumption: TransactionConsumption<S::Gas>,
        receipt: TransactionReceipt<S>,
    },
}

struct AuthAndProcessOutput<S: Spec, I: StateProvider<S>> {
    /// the *total* gas used in the course of authn and processing
    gas_used: <S as Spec>::Gas,
    scratchpad: TxScratchpad<S, I>,
    outcome: AuthAndProcessOutcome<S>,
}

#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
fn auth_and_process_tx<S, RT, I, C>(
    runtime: &RT,
    mut scratchpad: TxScratchpad<S, I>,
    raw_tx: &FullyBakedTx,
    sequencer_da_address: &<S::Da as DaSpec>::Address,
    gas_price: &<S::Gas as Gas>::Price,
    height: u64,
    execution_context: ExecutionContext,
    mut gas_meter: BasicGasMeter<<S as Spec>::Gas>,
    idx: usize,
    injected_control_flow: &C,
) -> AuthAndProcessOutput<S, I>
where
    S: Spec,
    RT: Runtime<S>,
    I: StateProvider<S>,
    C: InjectedControlFlow<TransactionReceipt<S>, S>,
{
    let mut pre_exec_working_set: PreExecWorkingSet<S, _> =
        scratchpad.to_pre_exec_working_set(gas_meter);

    // Charge gas for all the checks in the `process_tx`.
    // This can only fail when the number of transactions was *not* known up front (i.e. in the sequencer).
    // if the number of txs was known up front, we've already reserved sufficient gas during the pre-execution gsetp.
    if let Err(e) =
        pre_exec_working_set.charge_gas(&runtime.gas_enforcer().process_tx_pre_exec_checks_gas())
    {
        assert!(execution_context.is_sequencer(), "Attempted to run pre-execution checks without reserving sufficient gas. This is a bug! Please report it.");
        let gas_used = pre_exec_working_set.gas_info().gas_used;
        let (scratchpad, _gas_meter) = pre_exec_working_set.to_scratchpad_and_gas_meter();
        return AuthAndProcessOutput {
            outcome: AuthAndProcessOutcome::IllegalSequencer {
                reason: format!(
                    "The sequencer did not have sufficient funds to cover batch execution, {e}",
                ),
            },
            scratchpad,
            gas_used,
        };
    }

    let authentication_result =
        deserialize_and_authenticate(runtime, raw_tx, &mut pre_exec_working_set);

    let gas_used_for_authentication = pre_exec_working_set.gas_info().gas_used;

    let (next_scratchpad, returned_meter) = pre_exec_working_set.to_scratchpad_and_gas_meter();
    gas_meter = returned_meter;
    scratchpad = next_scratchpad;

    let (validated_output, gas_used_for_authentication) = match authentication_result {
        Ok(auth_output) => (
            ValidatedAuthOutput::Valid(auth_output),
            gas_used_for_authentication,
        ),
        Err(pre_exec_error) => match pre_exec_error {
            AuthenticationError::FatalError(err, tx_hash) => (
                ValidatedAuthOutput::Invalid {
                    tx_hash,
                    error: err,
                },
                gas_meter.gas_info().gas_used,
            ),
            AuthenticationError::OutOfGas(e) => {
                assert!(execution_context.is_sequencer(), "Attempted to run pre-execution checks without reserving sufficient gas. This is a bug! Please report it.");
                let gas_used = gas_meter.gas_info().gas_used;
                return AuthAndProcessOutput {
                        scratchpad,
                        gas_used,
                        outcome: AuthAndProcessOutcome::IllegalSequencer {
                            reason: format!("The sequencer did not have sufficient funds to cover batch execution: {}", e),
                        },
                    };
            }
        },
    };

    // Begin the transaction processing phase.
    let raw_tx_hash = validated_output.hash();
    let span = tracing::info_span!("transaction", id = %raw_tx_hash, idx = %idx).entered();

    let process_tx_result = process_tx(
        runtime,
        validated_output,
        gas_price,
        &gas_used_for_authentication,
        sequencer_da_address,
        height,
        scratchpad,
        execution_context,
        injected_control_flow,
    );

    span.exit();

    let (tx_result, next_scratchpad) = process_tx_result;

    match tx_result {
        Err(error) => {
            let gas_used = gas_used_for_authentication;
            AuthAndProcessOutput {
                outcome: AuthAndProcessOutcome::Skipped {
                    error,
                    tx_hash: raw_tx_hash,
                },
                scratchpad: next_scratchpad,
                gas_used,
            }
        }
        Ok(ApplyTxResult {
            transaction_consumption,
            receipt,
        }) => {
            let gas_used = get_gas_used(&receipt);
            AuthAndProcessOutput {
                gas_used,
                scratchpad: next_scratchpad,
                outcome: AuthAndProcessOutcome::Applied {
                    receipt,
                    transaction_consumption,
                },
            }
        }
    }
}
