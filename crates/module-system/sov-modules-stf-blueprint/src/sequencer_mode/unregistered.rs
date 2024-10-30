#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_cycle_utils::macros::cycle_tracker;
use sov_modules_api::capabilities::{
    GasEnforcer, SequencerRemuneration, TransactionAuthenticator, TransactionAuthorizer,
    TryReserveGasError, UnregisteredAuthenticationError,
};
use sov_modules_api::{
    BasicGasMeter, BatchSequencerOutcome, BatchSequencerReceipt, DaSpec, ExecutionContext, Gas,
    GasArray, GasInfo, GasMeter, PreExecWorkingSet, Rewards, Spec, TxScratchpad, WorkingSet,
};
use tracing::{debug, warn};

use crate::sequencer_mode::common::{
    apply_batch_logs, apply_tx, create_tx_receipt, get_gas_used, BatchReceipt, BEGIN_BATCH_HOOK_ERR,
};
use crate::{
    ApplyTxResult, AuthTxOutput, Runtime, SkippedTxContents, StateCheckpoint, TxProcessingError,
    ValidatedAuthOutput,
};

#[allow(clippy::result_large_err)]
pub fn process_unauthorized_tx<S: Spec, R: Runtime<S>>(
    runtime: &R,
    validated_output: ValidatedAuthOutput<S, R>,
    gas_info: GasInfo<S::Gas>,
    sequencer_da_address: &<S::Da as DaSpec>::Address,
    height: u64,
    mut scratchpad: TxScratchpad<S::Storage>,
    execution_context: ExecutionContext,
) -> (
    Result<ApplyTxResult<S>, TxProcessingError>,
    TxScratchpad<S::Storage>,
) {
    let (auth_tx, auth_data, message) = match validated_output {
        ValidatedAuthOutput::Valid(valid) => valid,
        ValidatedAuthOutput::Invalid { tx_hash, error } => {
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

    let mut ctx = match runtime
        .transaction_authorizer()
        .resolve_unregistered_context(
            &auth_data,
            sequencer_da_address,
            height,
            &mut scratchpad,
            execution_context,
        ) {
        Ok(ctx) => ctx,
        Err(e) => {
            return (
                Err(TxProcessingError::CannotResolveContext(e.to_string())),
                scratchpad,
            );
        }
    };

    // Check that the transaction isn't a duplicate
    if let Err(e) =
        runtime
            .transaction_authorizer()
            .check_uniqueness(&auth_data, &ctx, &mut scratchpad)
    {
        return (
            Err(TxProcessingError::IncorrectNonce(e.to_string())),
            scratchpad,
        );
    }

    if let Err(TryReserveGasError { reason }) =
        runtime
            .gas_enforcer()
            .try_reserve_gas(tx, &gas_info.gas_price, &mut ctx, &mut scratchpad)
    {
        return (
            Err(TxProcessingError::CannotReserveGas(reason.to_string())),
            scratchpad,
        );
    }

    let mut working_set = WorkingSet::create_working_set(scratchpad, &gas_info.gas_price, tx);

    // Here we charge the gas for the transaction sig & pre-execution checks.
    if let Err(err) = working_set.charge_gas(&gas_info.gas_used) {
        let (scratchpad, _) = working_set.revert();
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
    runtime.sequencer_remuneration().reward_sequencer_or_refund(
        sequencer_da_address,
        ctx.gas_refund_recipient(),
        sequencer_reward,
        &mut scratchpad,
    );

    (Ok(apply_tx), scratchpad)
}

#[allow(clippy::type_complexity)]
pub(crate) fn authenticate_unregistered_tx<S: Spec, R: Runtime<S>>(
    runtime: &R,
    meter: BasicGasMeter<S::Gas>,
    input: &<R as TransactionAuthenticator<S>>::Input,
    scratchpad: TxScratchpad<S::Storage>,
) -> (
    Result<(AuthTxOutput<S, R>, GasInfo<S::Gas>), UnregisteredAuthenticationError>,
    TxScratchpad<S::Storage>,
) {
    let mut pre_exec_working_set = scratchpad.to_pre_exec_working_set(meter);

    let res = authenticate_unregistered_with_cycle_count(runtime, input, &mut pre_exec_working_set);
    let (scratchpad, gas_meter) = pre_exec_working_set.to_scratchpad_and_gas_meter();

    match res {
        Err(e) => (Err(e), scratchpad),
        Ok(ok) => (Ok((ok, gas_meter.gas_info())), scratchpad),
    }
}

#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
fn authenticate_unregistered_with_cycle_count<S: Spec, R: Runtime<S>>(
    runtime: &R,
    input: &<R as TransactionAuthenticator<S>>::Input,
    pre_exec_working_set: &mut PreExecWorkingSet<S>,
) -> Result<AuthTxOutput<S, R>, UnregisteredAuthenticationError> {
    runtime.authenticate_unregistered(input, pre_exec_working_set)
}

pub(crate) struct BatchWithSingleTx<Input> {
    pub(crate) auth_input: Input,
    pub(crate) id: [u8; 32],
}

#[tracing::instrument(skip_all, name = "StfBlueprint::apply_batch")]
#[allow(clippy::too_many_arguments)]
#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
pub(crate) fn apply_batch<S, RT>(
    runtime: &RT,
    mut checkpoint: StateCheckpoint<S::Storage>,
    batch: BatchWithSingleTx<<RT as TransactionAuthenticator<S>>::Input>,
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
        batch_id = hex::encode(batch.id),
        sequencer_da_address = %sequencer_da_address,
        ?gas_price,
        "Applying a batch"
    );
    let mut scratchpad = checkpoint.to_tx_scratchpad();

    // The sequencer is not bonded so we don't charge for the batch hook gas.
    // ApplyBlobHook: begin
    if let Err(e) = runtime.begin_batch_hook(&sequencer_da_address, &mut scratchpad) {
        warn!(
            error = %e,
            batch_id = hex::encode(batch.id),
            BEGIN_BATCH_HOOK_ERR,
        );

        return (
            BatchReceipt {
                batch_hash: batch.id,
                tx_receipts: Vec::new(),
                inner: BatchSequencerReceipt {
                    da_address: sequencer_da_address,
                    gas_price: gas_price.clone(),
                    gas_used: S::Gas::zero(),
                    outcome: BatchSequencerOutcome::Ignored(BEGIN_BATCH_HOOK_ERR.to_string()),
                },
            },
            scratchpad.commit(),
        );
    }

    let mut tx_receipts = Vec::new();
    let mut gas_used = S::Gas::zero();
    let mut accumulated_reward = 0;

    debug!(
        batch_id = hex::encode(batch.id),
        "Verifying & executing transactions"
    );

    let max_auth_cost = runtime.gas_enforcer().max_tx_check_costs().value(gas_price);
    let meter = BasicGasMeter::new(max_auth_cost, gas_price.clone());

    let authentication_result =
        authenticate_unregistered_tx(runtime, meter, &batch.auth_input, scratchpad);

    let (validated_output, gas_info, scratchpad) = match authentication_result {
        (Ok((auth_output, gas_info)), scratchpad) => (
            ValidatedAuthOutput::Valid(auth_output),
            gas_info,
            scratchpad,
        ),
        (Err(UnregisteredAuthenticationError::FatalError(err, tx_hash)), scratchpad) => {
            warn!(error = ?err, "Authentication failed");
            (
                ValidatedAuthOutput::Invalid {
                    tx_hash,
                    error: err,
                },
                GasInfo {
                    // If the transaction is invalid `gas_used = S::Gas::zero()` because there is no one to charge (the sequencer is not bonded).
                    gas_used: S::Gas::zero(),
                    gas_price: gas_price.clone(),
                    remaining_funds: 0,
                },
                scratchpad,
            )
        }

        (Err(UnregisteredAuthenticationError::OutOfGas(err)), _) => {
            // It is safe to panic here because we have already confirmed that the gas is sufficient to authenticate the transaction.
            panic!(
                "The impossible happened: the sequencer ran out of gas {}.",
                err
            )
        }
    };

    let raw_tx_hash = validated_output.hash();

    let process_tx_result = process_unauthorized_tx(
        runtime,
        validated_output,
        gas_info,
        &sequencer_da_address,
        height,
        scratchpad,
        execution_context,
    );

    let (tx_result, mut scratchpad) = process_tx_result;

    match tx_result {
        Err(error) => {
            let skipped = SkippedTxContents {
                error,
                gas_used: S::Gas::zero(),
            };

            let tx_receipt = create_tx_receipt(skipped, raw_tx_hash, 0);
            tx_receipts.push(tx_receipt);
        }
        Ok(ApplyTxResult {
            transaction_consumption,
            receipt,
        }) => {
            // We reward sequencer only if the registration transaction is successful.
            if receipt.receipt.is_successful() {
                let sequencer_reward = transaction_consumption.priority_fee();
                accumulated_reward += sequencer_reward.0;
            }
            gas_used.combine(&get_gas_used(&receipt));
            tx_receipts.push(receipt);
        }
    }

    let batch_receipt = BatchReceipt {
        batch_hash: batch.id,
        tx_receipts,
        inner: BatchSequencerReceipt {
            da_address: sequencer_da_address,
            gas_price: gas_price.clone(),
            gas_used: gas_used.clone(),
            outcome: BatchSequencerOutcome::Executed(Rewards {
                accumulated_reward,
                accumulated_penalty: 0,
                // In the unregistered case, the sequencer does not cover the costs for the hooks.
                hooks_cost: 0,
            }),
        },
    };

    runtime.end_batch_hook(&batch_receipt.inner, &mut scratchpad);
    checkpoint = scratchpad.commit();

    apply_batch_logs(&batch_receipt, &gas_used, blob_idx);

    (batch_receipt, checkpoint)
}
