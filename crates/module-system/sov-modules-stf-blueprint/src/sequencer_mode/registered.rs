#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_cycle_utils::macros::cycle_tracker;
use sov_modules_api::capabilities::{
    fatal_deserialization_error, AuthenticationError, AuthenticationOutput,
    AuthorizeSequencerError, GasEnforcer, SequencerAuthorization, SequencerRemuneration,
    TransactionAuthenticator, TransactionAuthorizer, TryReserveGasError,
};
use sov_modules_api::runtime::capabilities::KernelSlotHooks;
use sov_modules_api::transaction::SequencerReward;
use sov_modules_api::{
    BasicGasMeter, BatchSequencerOutcome, BatchSequencerReceipt, BatchWithId, DaSpec,
    ExecutionContext, FullyBakedTx, Gas, GasArray, GasInfo, GasMeter, PreExecWorkingSet, Spec,
    StateCheckpoint, TxScratchpad, WorkingSet,
};
use tracing::{debug, error, warn};

pub use crate::sequencer_mode::common::PreExecError;
use crate::sequencer_mode::common::{
    apply_batch_logs, apply_tx, create_tx_receipt, get_gas_used, BatchReceipt, BEGIN_BATCH_HOOK_ERR,
};
use crate::{ApplyTxResult, AuthTxOutput, Runtime, SkippedTxContents, TxProcessingError};

/// Executes the entire transaction lifecycle.
#[allow(clippy::result_large_err)]
pub fn process_tx<S: Spec, R: Runtime<S>>(
    runtime: &R,
    auth_output: AuthenticationOutput<
        S,
        <R as TransactionAuthenticator<S>>::Decodable,
        <R as TransactionAuthenticator<S>>::AuthorizationData,
    >,
    gas_info: GasInfo<S::Gas>,
    sequencer_da_address: &<S::Da as DaSpec>::Address,
    height: u64,
    mut tx_scratchpad: TxScratchpad<S::Storage>,
    execution_context: ExecutionContext,
) -> (
    Result<ApplyTxResult<S>, TxProcessingError>,
    TxScratchpad<S::Storage>,
) {
    let (auth_tx, auth_data, message) = auth_output;

    let raw_tx_hash = auth_tx.raw_tx_hash;
    let tx = &auth_tx.authenticated_tx;

    let maybe_ctx = runtime.transaction_authorizer().resolve_context(
        &auth_data,
        sequencer_da_address,
        height,
        &mut tx_scratchpad,
        execution_context,
    );
    let ctx = match maybe_ctx {
        Ok(ctx) => ctx,
        Err(err) => {
            let err_string = err.to_string();

            // We penalize the sequencer for the fixed amount of gas that was used to execute the transaction.
            runtime.sequencer_authorization().penalize_sequencer(
                sequencer_da_address,
                err,
                gas_info.remaining_funds,
                &mut tx_scratchpad,
            );

            return (
                Err(TxProcessingError::CannotResolveContext(err_string)),
                tx_scratchpad,
            );
        }
    };

    // Check that the transaction isn't a duplicate
    if let Err(err) =
        runtime
            .transaction_authorizer()
            .check_uniqueness(&auth_data, &ctx, &mut tx_scratchpad)
    {
        let err_string = err.to_string();

        // We penalize the sequencer for the fixed amount of gas that was used to execute the transaction.
        runtime.sequencer_authorization().penalize_sequencer(
            sequencer_da_address,
            err,
            gas_info.remaining_funds,
            &mut tx_scratchpad,
        );

        return (
            Err(TxProcessingError::IncorrectNonce(err_string)),
            tx_scratchpad,
        );
    }

    if let Err(TryReserveGasError { reason }) =
        runtime
            .gas_enforcer()
            .try_reserve_gas(tx, &gas_info.gas_price, &ctx, &mut tx_scratchpad)
    {
        runtime.sequencer_authorization().penalize_sequencer(
            sequencer_da_address,
            &reason,
            gas_info.remaining_funds,
            &mut tx_scratchpad,
        );

        return (
            Err(TxProcessingError::CannotReserveGas(reason.to_string())),
            tx_scratchpad,
        );
    }

    let mut working_set = WorkingSet::create_working_set(tx_scratchpad, &gas_info.gas_price, tx);

    if let Err(err) = working_set.charge_gas(&gas_info.gas_used) {
        let (mut scratchpad, transaction_consumption) = working_set.revert();

        runtime.sequencer_authorization().penalize_sequencer(
            sequencer_da_address,
            &err,
            transaction_consumption.remaining_funds().0,
            &mut scratchpad,
        );

        return (
            Err(TxProcessingError::OutOfGas(err.to_string())),
            scratchpad,
        );
    }

    // If the transaction is valid, execute it and apply the changes to the state.
    let (apply_tx, mut tx_scratchpad) =
        apply_tx(runtime, &ctx, tx, raw_tx_hash, message, working_set);

    let transaction_consumption = &apply_tx.transaction_consumption;

    runtime.transaction_authorizer().mark_tx_attempted(
        &auth_data,
        sequencer_da_address,
        &mut tx_scratchpad,
    );

    runtime.gas_enforcer().refund_remaining_gas(
        ctx.sender(),
        &transaction_consumption.remaining_funds(),
        &mut tx_scratchpad,
    );

    runtime.gas_enforcer().reward_prover(
        &transaction_consumption.base_fee_value(),
        &mut tx_scratchpad,
    );

    let sequencer_reward = transaction_consumption.priority_fee();
    runtime.sequencer_remuneration().reward_sequencer(
        sequencer_da_address,
        sequencer_reward,
        &mut tx_scratchpad,
    );

    (Ok(apply_tx), tx_scratchpad)
}

/// Authenticate the transaction from the (supposedly) registered sequencer before execution
#[allow(clippy::type_complexity)]
pub fn authenticate_tx<S: Spec, R: Runtime<S>>(
    runtime: &R,
    gas_price: &<S::Gas as Gas>::Price,
    sequencer_da_address: &<S::Da as DaSpec>::Address,
    tx: &FullyBakedTx,
    mut scratchpad: TxScratchpad<S::Storage>,
) -> (
    Result<(AuthTxOutput<S, R>, GasInfo<S::Gas>), PreExecError>,
    TxScratchpad<S::Storage>,
) {
    // Checks the sequencer balance before the transaction is executed.
    // If the sequencer balance is not high enough, the transaction is rejected.
    let gas_meter = match runtime.sequencer_authorization().authorize_sequencer(
        sequencer_da_address,
        gas_price,
        &mut scratchpad,
    ) {
        Ok(allowed_seqiencer) => BasicGasMeter::new(allowed_seqiencer.balance, gas_price.clone()),
        Err(AuthorizeSequencerError { reason }) => {
            return (Err(PreExecError::SequencerError(reason)), scratchpad);
        }
    };

    let mut pre_exec_working_set = scratchpad.to_pre_exec_working_set(gas_meter);
    let res = authenticate_with_cycle_count(runtime, tx, &mut pre_exec_working_set);

    let gas_info = pre_exec_working_set.gas_info();
    let (mut tx_scratchpad, _) = pre_exec_working_set.to_scratchpad_and_gas_meter();
    match res {
        Err(e @ AuthenticationError::FatalError(_, _)) => {
            runtime.sequencer_authorization().penalize_sequencer(
                sequencer_da_address,
                e.clone(),
                gas_info.remaining_funds,
                &mut tx_scratchpad,
            );

            (Err(PreExecError::AuthError(e)), tx_scratchpad)
        }
        Err(e @ AuthenticationError::OutOfGas(_)) => {
            (Err(PreExecError::AuthError(e)), tx_scratchpad)
        }
        Ok(ok) => (Ok((ok, gas_info)), tx_scratchpad),
    }
}

#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
fn authenticate_with_cycle_count<S: Spec, R: Runtime<S>>(
    runtime: &R,
    tx: &FullyBakedTx,
    pre_exec_working_set: &mut PreExecWorkingSet<S>,
) -> Result<AuthTxOutput<S, R>, AuthenticationError> {
    let auth_input = borsh::from_slice(&tx.data)
        .map_err(|e| fatal_deserialization_error::<S, _>(&tx.data, e, pre_exec_working_set))?;
    runtime.authenticate(&auth_input, pre_exec_working_set)
}

#[tracing::instrument(skip_all, name = "StfBlueprint::apply_batch")]
#[allow(clippy::too_many_arguments)]
#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
pub(crate) fn apply_batch<S, RT, K>(
    runtime: &RT,
    mut checkpoint: StateCheckpoint<S::Storage>,
    batch_with_id: BatchWithId,
    blob_idx: usize,
    sequencer_da_address: <S::Da as DaSpec>::Address,
    gas_price: &<S::Gas as Gas>::Price,
    height: u64,
    execution_context: ExecutionContext,
) -> (BatchReceipt<S>, StateCheckpoint<S::Storage>, S::Gas)
where
    S: Spec,
    RT: Runtime<S>,
    K: KernelSlotHooks<S>,
{
    debug!(
        batch_id = hex::encode(batch_with_id.id),
        sequencer_da_address = %sequencer_da_address,
        ?gas_price,
        "Applying a batch"
    );

    // ApplyBlobHook: begin
    if let Err(e) = runtime.begin_batch_hook(&sequencer_da_address, &mut checkpoint) {
        error!(
            error = %e,
            batch_id = hex::encode(batch_with_id.id),
            BEGIN_BATCH_HOOK_ERR,
        );

        return (
            BatchReceipt {
                batch_hash: batch_with_id.id,
                tx_receipts: Vec::new(),
                inner: BatchSequencerReceipt {
                    da_address: sequencer_da_address,
                    outcome: BatchSequencerOutcome::Ignored(BEGIN_BATCH_HOOK_ERR.to_string()),
                },
                gas_price: gas_price.clone(),
            },
            checkpoint,
            S::Gas::zero(),
        );
    }

    let raw_txs = batch_with_id.batch.txs;

    let mut tx_receipts = Vec::with_capacity(raw_txs.len());
    let mut gas_used = S::Gas::zero();
    let mut accumulated_reward = SequencerReward::ZERO;

    debug!(
        batch_id = hex::encode(batch_with_id.id),
        txs_num = raw_txs.len(),
        "Verifying & executing transactions"
    );

    for (idx, raw_tx) in raw_txs.iter().enumerate() {
        let tx_scratchpad = checkpoint.to_tx_scratchpad();

        let authentication_result = authenticate_tx(
            runtime,
            gas_price,
            &sequencer_da_address,
            raw_tx,
            tx_scratchpad,
        );

        let (auth_output, gas_info, tx_scratchpad) = match authentication_result {
            (Ok((auth_output, gas_info)), tx_scratchpad) => (auth_output, gas_info, tx_scratchpad),
            (Err(pre_exec_error), scratchpad) => match pre_exec_error {
                PreExecError::SequencerError(error) => {
                    let remaining = raw_txs.len() - idx - 1;
                    error!(
                        reason = %error,
                        sequencer_da_address = %sequencer_da_address,
                        tx_idx = %idx,
                        remaining = remaining,
                        "The transaction was rejected by the 'authorize_sequencer' capability. Dropping the remaining transactions in that batch",
                    );

                    return (
                        BatchReceipt {
                            batch_hash: batch_with_id.id,
                            tx_receipts,
                            inner: BatchSequencerReceipt {
                                da_address: sequencer_da_address,
                                outcome: BatchSequencerOutcome::Rewarded(accumulated_reward),
                            },
                            gas_price: gas_price.clone(),
                        },
                        scratchpad.commit(),
                        gas_used,
                    );
                }
                PreExecError::AuthError(e) => match e {
                    AuthenticationError::FatalError(err, _) => {
                        error!(
                            sequencer_da_address = %sequencer_da_address,
                            err=%err, "Tx authentication raised a fatal error, sequencer slashed");

                        return (
                            BatchReceipt {
                                batch_hash: batch_with_id.id,
                                tx_receipts,
                                inner: BatchSequencerReceipt {
                                    da_address: sequencer_da_address,
                                    outcome: BatchSequencerOutcome::Ignored(err.to_string()),
                                },
                                gas_price: gas_price.clone(),
                            },
                            scratchpad.commit(),
                            gas_used,
                        );
                    }
                    AuthenticationError::OutOfGas(err) => {
                        // TODO: https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/901
                        error!(error = ?err, "Transaction will be completely forgotten, just like tears in the rain.");
                        checkpoint = scratchpad.commit();
                        continue;
                    }
                },
            },
        };

        let raw_tx_hash = auth_output.0.raw_tx_hash;

        let process_tx_result = process_tx(
            runtime,
            auth_output,
            gas_info,
            &sequencer_da_address,
            height,
            tx_scratchpad,
            execution_context,
        );

        let (tx_result, tx_scratchpad) = process_tx_result;
        checkpoint = tx_scratchpad.commit();
        match tx_result {
            Err(error) => {
                let skipped = SkippedTxContents {
                    error,
                    gas_used: S::Gas::zero(),
                };

                let tx_receipt = create_tx_receipt(skipped, raw_tx_hash, idx);
                tx_receipts.push(tx_receipt);
            }
            Ok(ApplyTxResult {
                transaction_consumption,
                receipt,
            }) => {
                gas_used.combine(&get_gas_used(&receipt));
                tx_receipts.push(receipt);

                let sequencer_reward = transaction_consumption.priority_fee();
                accumulated_reward.accumulate(sequencer_reward);
            }
        }
    }

    let batch_receipt = BatchReceipt {
        batch_hash: batch_with_id.id,
        tx_receipts,
        inner: BatchSequencerReceipt {
            da_address: sequencer_da_address,
            outcome: BatchSequencerOutcome::Rewarded(accumulated_reward),
        },
        gas_price: gas_price.clone(),
    };

    runtime.end_batch_hook(&batch_receipt.inner, &mut checkpoint);

    apply_batch_logs(&batch_receipt, &gas_used, blob_idx);

    (batch_receipt, checkpoint, gas_used)
}
