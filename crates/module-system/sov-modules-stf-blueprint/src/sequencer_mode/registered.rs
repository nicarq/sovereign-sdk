#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_cycle_utils::macros::cycle_tracker;
use sov_modules_api::capabilities::{
    fatal_deserialization_error, AuthenticationError, AuthorizeSequencerError, GasEnforcer,
    SequencerAuthorization, SequencerRemuneration, TransactionAuthorizer, TryReserveGasError,
};
use sov_modules_api::transaction::SequencerReward;
use sov_modules_api::{
    BasicGasMeter, BatchSequencerOutcome, BatchSequencerReceipt, BatchWithId, DaSpec,
    ExecutionContext, FullyBakedTx, Gas, GasArray, GasMeter, PreExecWorkingSet, Spec,
    StateCheckpoint, TxScratchpad, WorkingSet,
};
use sov_rollup_interface::TxHash;
use tracing::{debug, error, warn};

pub use crate::sequencer_mode::common::PreExecError;
use crate::sequencer_mode::common::{
    apply_batch_logs, apply_tx, create_tx_receipt, get_gas_used, BatchReceipt, BEGIN_BATCH_HOOK_ERR,
};
use crate::{ApplyTxResult, AuthTxOutput, Runtime, SkippedTxContents, TxProcessingError};

/// Executes the entire transaction lifecycle.

#[allow(clippy::result_large_err, clippy::too_many_arguments)]
pub fn process_tx<S: Spec, R: Runtime<S>>(
    runtime: &R,
    validated_output: ValidatedAuthOutput<S, R>,
    gas_price: &<S::Gas as Gas>::Price,
    gas_used_for_authentication: &S::Gas,
    sequencer_da_address: &<S::Da as DaSpec>::Address,
    height: u64,
    mut scratchpad: TxScratchpad<S::Storage>,
    execution_context: ExecutionContext,
) -> (
    Result<ApplyTxResult<S>, TxProcessingError>,
    TxScratchpad<S::Storage>,
) {
    let auth_cost = gas_used_for_authentication.value(gas_price);

    let penalize = |tx_scratchpad: &mut TxScratchpad<S::Storage>| {
        runtime
            .gas_enforcer()
            .transfer_funds_from_sequencer_to_prover(auth_cost, sequencer_da_address, tx_scratchpad)
            // We ensured this before entering the tx execution loop.
            .expect("Sequencer should have enough funds to pay for the pre-execution checks");
    };

    let (auth_tx, auth_data, message) = match validated_output {
        ValidatedAuthOutput::Valid(valid) => valid,
        ValidatedAuthOutput::Invalid(hex_string) => {
            penalize(&mut scratchpad);

            return (
                Err(TxProcessingError::AuthenticationFailed(format!(
                    "Authentication failed for tx: {}",
                    hex_string
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
        let (mut scratchpad, _transaction_consumption) = working_set.revert();
        penalize(&mut scratchpad);

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
pub(crate) fn apply_batch<S, RT>(
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
{
    debug!(
        batch_id = hex::encode(batch_with_id.id),
        sequencer_da_address = %sequencer_da_address,
        ?gas_price,
        "Applying a batch"
    );

    let mut scratchpad = checkpoint.to_tx_scratchpad();

    let ignored_batch = |reason, seq_da_address| BatchReceipt {
        batch_hash: batch_with_id.id,
        tx_receipts: Vec::new(),
        inner: BatchSequencerReceipt {
            da_address: seq_da_address,
            outcome: BatchSequencerOutcome::Ignored(reason),
        },
        gas_price: gas_price.clone(),
    };

    let batch_hook_gas = runtime.gas_enforcer().batch_hook_gas();

    // Charge gas for batch hooks.
    match runtime
        .gas_enforcer()
        .transfer_funds_from_sequencer_to_prover(
            batch_hook_gas.value(gas_price),
            &sequencer_da_address,
            &mut scratchpad,
        ) {
        Ok(_) => (),
        Err(e) => {
            let err_str = format!("Not enough gas to execute `begin_batch_hook`: {}", e);
            error!(
                error = %e,
                batch_id = hex::encode(batch_with_id.id),
                "Not enough gas to execute `begin_batch_hook` ",
            );

            return (
                ignored_batch(err_str, sequencer_da_address),
                scratchpad.revert(),
                S::Gas::zero(),
            );
        }
    }

    let mut gas_used = batch_hook_gas;

    // ApplyBlobHook: begin
    if let Err(e) = runtime.begin_batch_hook(&sequencer_da_address, &mut scratchpad) {
        error!(
            error = %e,
            batch_id = hex::encode(batch_with_id.id),
            BEGIN_BATCH_HOOK_ERR,
        );

        return (
            ignored_batch(BEGIN_BATCH_HOOK_ERR.to_string(), sequencer_da_address),
            scratchpad.revert(),
            gas_used,
        );
    }
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
                ),
                scratchpad.commit(),
                gas_used,
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

            error!(
                error = %reason,
                batch_id = hex::encode(batch_with_id.id),
                "Not enough gas to authenticate the batch",
            );

            return (
                ignored_batch(err_str, sequencer_da_address),
                scratchpad.commit(),
                gas_used,
            );
        }
    };

    let mut pre_exec_working_set: PreExecWorkingSet<S> =
        scratchpad.to_pre_exec_working_set(gas_meter);

    let mut auth_outputs: Vec<(usize, ValidatedAuthOutput<S, RT>, S::Gas)> = Vec::new();

    let mut tx_receipts = Vec::with_capacity(raw_txs.len());
    let mut accumulated_reward = SequencerReward::ZERO;

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
                AuthenticationError::FatalError(err, hash) => {
                    error!(error = ?err, "Authentication failed");
                    auth_outputs.push((
                        idx,
                        ValidatedAuthOutput::Invalid(hash),
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

        match tx_result {
            Err(error) => {
                tracing::info!(
                    sequencer = %sequencer_da_address,
                    reason = %error,
                    "The sequencer paid for the transaction.",
                );

                let skipped = SkippedTxContents {
                    error,
                    gas_used: gas_used_for_authentication,
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
    // End of the transaction processing phase.

    let batch_receipt = BatchReceipt {
        batch_hash: batch_with_id.id,
        tx_receipts,
        inner: BatchSequencerReceipt {
            da_address: sequencer_da_address,
            outcome: BatchSequencerOutcome::Rewarded(accumulated_reward),
        },
        gas_price: gas_price.clone(),
    };

    runtime.end_batch_hook(&batch_receipt.inner, &mut batch_scratchpad);
    checkpoint = batch_scratchpad.commit();
    apply_batch_logs(&batch_receipt, &gas_used, blob_idx);

    (batch_receipt, checkpoint, gas_used)
}

/// The output of the authentication phase.
pub enum ValidatedAuthOutput<S: Spec, R: Runtime<S>> {
    /// Transaction data after the authentication phase.
    Valid(AuthTxOutput<S, R>),
    /// Hash of the invalid transaction.
    Invalid(TxHash),
}

impl<S: Spec, R: Runtime<S>> ValidatedAuthOutput<S, R> {
    /// Get hash of the Validated Auth Output.
    pub fn hash(&self) -> TxHash {
        match self {
            ValidatedAuthOutput::Valid(valid) => valid.0.raw_tx_hash,
            ValidatedAuthOutput::Invalid(hash) => *hash,
        }
    }
}
