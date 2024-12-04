#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_cycle_utils::macros::cycle_tracker;
use sov_modules_api::capabilities::{
    fatal_deserialization_error, AuthenticationError, AuthorizeSequencerError, GasEnforcer,
    SequencerAuthorization, SequencerRemuneration, TransactionAuthorizer, TryReserveGasError,
};
#[cfg(feature = "native")]
use sov_modules_api::NestedEnumUtils;
use sov_modules_api::{
    BasicGasMeter, BatchSequencerOutcome, BatchSequencerReceipt, BatchWithId, DaSpec,
    ExecutionContext, FullyBakedTx, Gas, GasArray, GasMeter, PreExecWorkingSet, Rewards, Spec,
    StateCheckpoint, StateProvider, TxScratchpad, WorkingSet,
};
use tracing::{debug, warn};

use super::common::ValidatedAuthOutput;
pub use crate::sequencer_mode::common::PreExecError;
use crate::sequencer_mode::common::{
    apply_batch_logs, apply_tx, create_tx_receipt, get_gas_used, BatchReceipt,
};
use crate::{ApplyTxResult, AuthTxOutput, Runtime, SkippedTxContents, TxProcessingError};

/// Executes the entire transaction lifecycle.
#[allow(clippy::result_large_err, clippy::too_many_arguments)]
pub fn process_tx<S: Spec, R: Runtime<S>, I: StateProvider<S>>(
    runtime: &R,
    validated_output: ValidatedAuthOutput<S, R>,
    gas_price: &<S::Gas as Gas>::Price,
    gas_used_for_authentication: &S::Gas,
    sequencer_da_address: &<S::Da as DaSpec>::Address,
    height: u64,
    scratchpad: TxScratchpad<S, I>,
    execution_context: ExecutionContext,
) -> (
    Result<ApplyTxResult<S>, TxProcessingError>,
    TxScratchpad<S, I>,
) {
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
fn process_tx_inner<S: Spec, R: Runtime<S>, I: StateProvider<S>>(
    runtime: &R,
    validated_output: ValidatedAuthOutput<S, R>,
    gas_price: &<S::Gas as Gas>::Price,
    gas_used_for_authentication: &S::Gas,
    sequencer_da_address: &<S::Da as DaSpec>::Address,
    height: u64,
    mut scratchpad: TxScratchpad<S, I>,
    execution_context: ExecutionContext,
) -> (
    Result<ApplyTxResult<S>, TxProcessingError>,
    TxScratchpad<S, I>,
) {
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
fn authenticate_with_cycle_count<S: Spec, R: Runtime<S>, I: StateProvider<S>>(
    runtime: &R,
    tx: &FullyBakedTx,
    pre_exec_working_set: &mut PreExecWorkingSet<S, I>,
) -> Result<AuthTxOutput<S, R>, AuthenticationError> {
    let auth_input = borsh::from_slice(&tx.data).map_err(|e| {
        fatal_deserialization_error::<PreExecWorkingSet<S, I>, S, _>(
            &tx.data,
            e,
            pre_exec_working_set,
        )
    })?;
    runtime.authenticate(&auth_input, pre_exec_working_set)
}

#[tracing::instrument(skip_all, name = "StfBlueprint::apply_batch")]
#[allow(clippy::too_many_arguments)]
#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
pub(crate) fn apply_batch<S, RT>(
    runtime: &RT,
    mut checkpoint: StateCheckpoint<S::Storage>,
    batch_with_id: BatchWithId,
    blob_idx: usize,
    sequencer_da_address: <S::Da as DaSpec>::Address,
    gas_price: &<S::Gas as Gas>::Price,
    height: u64,
    execution_context: ExecutionContext,
) -> (BatchReceipt<S>, StateCheckpoint<S::Storage>)
where
    S: Spec,
    RT: Runtime<S>,
{
    debug!(
        batch_id = hex::encode(batch_with_id.id),
        sequencer_da_address = %sequencer_da_address,
        ?gas_price,
        "Applying a batch"
    );

    let mut scratchpad = checkpoint.to_tx_scratchpad();

    let ignored_batch = |reason, seq_da_address, gas_used| BatchReceipt {
        batch_hash: batch_with_id.id,
        tx_receipts: Vec::new(),
        inner: BatchSequencerReceipt {
            da_address: seq_da_address,
            gas_price: gas_price.clone(),
            gas_used,
            outcome: BatchSequencerOutcome::Ignored(reason),
        },
    };

    let batch_hook_gas = runtime.gas_enforcer().batch_hook_gas();
    let batch_hook_gas_value = batch_hook_gas.value(gas_price);

    // Charge gas for batch hooks.
    match runtime
        .gas_enforcer()
        .transfer_funds_from_sequencer_to_prover(
            batch_hook_gas_value,
            &sequencer_da_address,
            &mut scratchpad,
        ) {
        Ok(_) => (),
        Err(e) => {
            let err_str = format!("Not enough gas to execute `begin_batch_hook`: {}", e);
            warn!(
                error = %e,
                batch_id = hex::encode(batch_with_id.id),
                "Not enough gas to execute `begin_batch_hook` ",
            );

            return (
                ignored_batch(err_str, sequencer_da_address, S::Gas::zero()),
                scratchpad.revert(),
            );
        }
    }

    let mut gas_used = batch_hook_gas;

    let raw_txs = batch_with_id.batch.txs;

    debug!(
        batch_id = hex::encode(batch_with_id.id),
        txs_num = raw_txs.len(),
        "Verifying & executing transactions"
    );

    // Cost of the authentication for the entire batch.
    // It should include the costs of `authentication` and process_tx pre-execution checks.
    let max_batch_check_costs = match runtime
        .gas_enforcer()
        .max_tx_check_costs()
        .value(gas_price)
        .checked_mul(raw_txs.len() as u64)
    {
        Some(cost) => cost,
        None => {
            return (
                ignored_batch(
                    "The calculation of the maximum authentication cost resulted in an overflow"
                        .to_string(),
                    sequencer_da_address,
                    gas_used.clone(),
                ),
                scratchpad.commit(),
            );
        }
    };

    // Begin the transaction authorization phase.
    let gas_meter: BasicGasMeter<S::Gas> = match runtime
        .sequencer_authorization()
        .authorize_sequencer(
            &sequencer_da_address,
            max_batch_check_costs,
            &mut scratchpad,
        ) {
        Ok(allowed_sequencer) => BasicGasMeter::new(allowed_sequencer.balance, gas_price.clone()),
        Err(AuthorizeSequencerError { reason }) => {
            let err_str = format!("Not enough gas to authenticate the batch: {}", reason);

            warn!(
                error = %reason,
                batch_id = hex::encode(batch_with_id.id),
                "Not enough gas to authenticate the batch",
            );

            return (
                ignored_batch(err_str, sequencer_da_address, gas_used.clone()),
                scratchpad.commit(),
            );
        }
    };

    let mut pre_exec_working_set: PreExecWorkingSet<S, _> =
        scratchpad.to_pre_exec_working_set(gas_meter);

    let mut auth_outputs: Vec<(usize, ValidatedAuthOutput<S, RT>, S::Gas)> = Vec::new();

    let mut tx_receipts = Vec::with_capacity(raw_txs.len());
    let mut accumulated_reward = 0;
    let mut accumulated_penalty = 0;

    for (idx, raw_tx) in raw_txs.iter().enumerate() {
        pre_exec_working_set.start_recording_gas_usage();

        // Charge gas for all the checks in the `process_tx`.
        pre_exec_working_set
            .charge_gas(&runtime.gas_enforcer().process_tx_pre_exec_checks_gas())
            // It is safe to `expect`` here because we have already confirmed that the gas is sufficient to execute the entire batch
            // when we ensured that the sequencer's staked balance is more than the `max_tx_check_costs`.
            // WARNING:
            //   This will break the rollup if the gas constants are not set correctly.
            //   Please ensure to thoroughly test edge cases and ensure that the sequencer cannot run out of gas during pre-processing checks.
            .expect("The impossible happened: the sequencer ran out of gas {}.");

        let authentication_result =
            authenticate_with_cycle_count(runtime, raw_tx, &mut pre_exec_working_set);

        let gas_used_for_authentication = pre_exec_working_set.get_recorded_gas_usage();

        match authentication_result {
            Ok(auth_output) => {
                auth_outputs.push((
                    idx,
                    ValidatedAuthOutput::Valid(auth_output),
                    gas_used_for_authentication,
                ));
            }
            Err(pre_exec_error) => match pre_exec_error {
                AuthenticationError::FatalError(err, tx_hash) => {
                    warn!(error = ?err, "Authentication failed");
                    auth_outputs.push((
                        idx,
                        ValidatedAuthOutput::Invalid {
                            tx_hash,
                            error: err,
                        },
                        gas_used_for_authentication,
                    ));
                }
                AuthenticationError::OutOfGas(err) => {
                    // It is safe to panic here because we have already confirmed that the gas is sufficient to execute the entire batch
                    // when we ensured that the sequencer's staked balance is more than the `max_tx_check_costs`.
                    // WARNING:
                    //   This will break the rollup if the gas constants are not set correctly.
                    //   Please ensure to thoroughly test edge cases and ensure that the sequencer cannot run out of gas during pre-processing checks.
                    panic!(
                        "The impossible happened: the sequencer ran out of gas {}.",
                        err
                    )
                }
            },
        }
    }
    // End of the transaction authorization phase.

    let (mut batch_scratchpad, _) = pre_exec_working_set.to_scratchpad_and_gas_meter();

    // Begin the transaction processing phase.
    for (idx, validated_output, gas_used_for_authentication) in auth_outputs.into_iter() {
        let raw_tx_hash = validated_output.hash();

        let process_tx_result = process_tx(
            runtime,
            validated_output,
            gas_price,
            &gas_used_for_authentication,
            &sequencer_da_address,
            height,
            batch_scratchpad,
            execution_context,
        );

        let (tx_result, next_scratchpad) = process_tx_result;
        batch_scratchpad = next_scratchpad;

        let receipt = match tx_result {
            Err(error) => {
                tracing::info!(
                    sequencer = %sequencer_da_address,
                    reason = %error,
                    "The sequencer paid for the transaction.",
                );

                accumulated_penalty += gas_used_for_authentication.value(gas_price);
                gas_used.combine(&gas_used_for_authentication);

                let skipped = SkippedTxContents {
                    error,
                    gas_used: gas_used_for_authentication,
                };

                create_tx_receipt(skipped, raw_tx_hash, idx)
            }
            Ok(ApplyTxResult {
                transaction_consumption,
                receipt,
            }) => {
                gas_used.combine(&get_gas_used(&receipt));

                let sequencer_reward = transaction_consumption.priority_fee();
                accumulated_reward += sequencer_reward.0;
                receipt
            }
        };
        tx_receipts.push(receipt);
    }
    // End of the transaction processing phase.

    let batch_receipt = BatchReceipt {
        batch_hash: batch_with_id.id,
        tx_receipts,
        inner: BatchSequencerReceipt {
            da_address: sequencer_da_address,
            gas_price: gas_price.clone(),
            gas_used: gas_used.clone(),
            outcome: BatchSequencerOutcome::Executed(Rewards {
                accumulated_reward,
                accumulated_penalty,
                hooks_cost: batch_hook_gas_value,
            }),
        },
    };

    checkpoint = batch_scratchpad.commit();
    apply_batch_logs(&batch_receipt, &gas_used, blob_idx);

    (batch_receipt, checkpoint)
}
