#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_cycle_utils::macros::cycle_tracker;
use sov_modules_api::capabilities::{
    calculate_hash, AuthenticationOutput, FatalError, GasEnforcer, SequencerRemuneration,
    TransactionAuthenticator, TransactionAuthorizer, TryReserveGasError,
    UnregisteredAuthenticationError,
};
use sov_modules_api::transaction::SequencerReward;
use sov_modules_api::{
    BasicGasMeter, BatchSequencerOutcome, BatchSequencerReceipt, DaSpec, ExecutionContext,
    FullyBakedTx, Gas, GasArray, GasInfo, GasMeter, PreExecWorkingSet, Spec, TxScratchpad,
    WorkingSet,
};
use tracing::{debug, error, warn};

use crate::sequencer_mode::common::{
    apply_batch_logs, apply_tx, create_tx_receipt, get_gas_used, BatchReceipt, BEGIN_BATCH_HOOK_ERR,
};
use crate::{
    ApplyTxResult, AuthTxOutput, Runtime, SkippedTxContents, StateCheckpoint, TxProcessingError,
};

#[allow(clippy::result_large_err)]
pub fn process_unauthorized_tx<S: Spec, R: Runtime<S>>(
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

    let ctx = match runtime
        .transaction_authorizer()
        .resolve_unregistered_context(&auth_data, height, &mut tx_scratchpad, execution_context)
    {
        Ok(ctx) => ctx,
        Err(e) => {
            return (
                Err(TxProcessingError::CannotResolveContext(e.to_string())),
                tx_scratchpad,
            );
        }
    };

    // Check that the transaction isn't a duplicate
    if let Err(e) =
        runtime
            .transaction_authorizer()
            .check_uniqueness(&auth_data, &ctx, &mut tx_scratchpad)
    {
        return (
            Err(TxProcessingError::IncorrectNonce(e.to_string())),
            tx_scratchpad,
        );
    }

    if let Err(TryReserveGasError { reason }) =
        runtime
            .gas_enforcer()
            .try_reserve_gas(tx, &gas_info.gas_price, &ctx, &mut tx_scratchpad)
    {
        return (
            Err(TxProcessingError::CannotReserveGas(reason.to_string())),
            tx_scratchpad,
        );
    }

    let mut working_set = WorkingSet::create_working_set(tx_scratchpad, &gas_info.gas_price, tx);

    if let Err(err) = working_set.charge_gas(&gas_info.gas_used) {
        let (scratchpad, _) = working_set.revert();
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

#[allow(clippy::type_complexity)]
pub(crate) fn authenticate_unregistered_tx<S: Spec, R: Runtime<S>>(
    runtime: &R,
    gas_price: &<S::Gas as Gas>::Price,
    tx: &FullyBakedTx,
    tx_scratchpad: TxScratchpad<S::Storage>,
) -> (
    Result<(AuthTxOutput<S, R>, GasInfo<S::Gas>), UnregisteredAuthenticationError>,
    TxScratchpad<S::Storage>,
) {
    // TODO #1490: Remove u64::MAX
    let meter = BasicGasMeter::new(u64::MAX, gas_price.clone());
    let mut pre_exec_working_set = tx_scratchpad.to_pre_exec_working_set(meter);

    let res = authenticate_unregistered_with_cycle_count(runtime, tx, &mut pre_exec_working_set);
    let (tx_scratchpad, gas_meter) = pre_exec_working_set.to_scratchpad_and_gas_meter();

    match res {
        Err(e) => (Err(e), tx_scratchpad),
        Ok(ok) => (Ok((ok, gas_meter.gas_info())), tx_scratchpad),
    }
}

#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
fn authenticate_unregistered_with_cycle_count<S: Spec, R: Runtime<S>>(
    runtime: &R,
    tx: &FullyBakedTx,
    pre_exec_working_set: &mut PreExecWorkingSet<S>,
) -> Result<AuthTxOutput<S, R>, UnregisteredAuthenticationError> {
    let auth_input = borsh::from_slice(&tx.data).map_err(|e| {
        match calculate_hash::<S>(&tx.data, pre_exec_working_set) {
            Ok(hash) => UnregisteredAuthenticationError::FatalError(
                FatalError::DeserializationFailed(e.to_string()),
                hash,
            ),
            Err(err) => UnregisteredAuthenticationError::OutOfGas(err.to_string()),
        }
    })?;
    runtime.authenticate_unregistered(&auth_input, pre_exec_working_set)
}

pub(crate) struct BatchWithSingleTx {
    pub(crate) fully_baked_tx: FullyBakedTx,
    pub(crate) id: [u8; 32],
}

#[tracing::instrument(skip_all, name = "StfBlueprint::apply_batch")]
#[allow(clippy::too_many_arguments)]
#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
pub(crate) fn apply_batch<S, RT>(
    runtime: &RT,
    mut checkpoint: StateCheckpoint<S::Storage>,
    batch: BatchWithSingleTx,
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
        batch_id = hex::encode(batch.id),
        sequencer_da_address = %sequencer_da_address,
        ?gas_price,
        "Applying a batch"
    );

    // ApplyBlobHook: begin
    if let Err(e) = runtime.begin_batch_hook(&sequencer_da_address, &mut checkpoint) {
        error!(
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
                    outcome: BatchSequencerOutcome::Ignored(BEGIN_BATCH_HOOK_ERR.to_string()),
                },
                gas_price: gas_price.clone(),
            },
            checkpoint,
            S::Gas::zero(),
        );
    }

    let mut tx_receipts = Vec::new();
    let mut gas_used = S::Gas::zero();
    let mut accumulated_reward = SequencerReward::ZERO;

    debug!(
        batch_id = hex::encode(batch.id),
        "Verifying & executing transactions"
    );

    let tx_scratchpad = checkpoint.to_tx_scratchpad();

    let authentication_result =
        authenticate_unregistered_tx(runtime, gas_price, &batch.fully_baked_tx, tx_scratchpad);

    let (auth_output, gas_info, tx_scratchpad) = match authentication_result {
        (Ok((auth_output, gas_info)), tx_scratchpad) => (auth_output, gas_info, tx_scratchpad),
        (Err(err), scratchpad) => {
            warn!(
                sequencer_da_address = %sequencer_da_address,
                reason = %err,
                "Processing of unregistered sequencer transaction raised error, skipping"
            );

            return (
                BatchReceipt {
                    batch_hash: batch.id,
                    tx_receipts: Vec::new(),
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
    };

    let raw_tx_hash = auth_output.0.raw_tx_hash;

    let process_tx_result = process_unauthorized_tx(
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

            let tx_receipt = create_tx_receipt(skipped, raw_tx_hash, 0);
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

    let batch_receipt = BatchReceipt {
        batch_hash: batch.id,
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
