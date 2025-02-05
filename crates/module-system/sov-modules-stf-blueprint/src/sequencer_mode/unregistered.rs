use sov_modules_api::capabilities::{
    BatchFromUnregisteredSequencer, GasEnforcer, SequencerRemuneration, TransactionAuthorizer,
    TryReserveGasError, UnregisteredAuthenticationError,
};
use sov_modules_api::{
    BasicGasMeter, BatchSequencerOutcome, BatchSequencerReceipt, DaSpec, Gas, GasArray, GasInfo,
    GasMeter, GasSpec, GetGasInfo, IgnoredTransactionReceipt, Rewards, Spec, StateProvider,
    TxScratchpad, WorkingSet,
};
use tracing::{debug, warn};

use crate::sequencer_mode::common::{
    apply_batch_logs, apply_tx, create_tx_receipt, get_gas_used, BatchReceipt,
};
use crate::{
    ApplyTxResult, AuthTxOutput, IgnoredTxContents, Runtime, SkippedTxContents, StateCheckpoint,
    TransactionReceipt, TxProcessingError, TxReceiptContents,
};

#[allow(clippy::result_large_err)]
#[allow(clippy::too_many_arguments)]
pub fn process_unauthorized_tx<S: Spec, R: Runtime<S>, I: StateProvider<S>>(
    runtime: &R,
    slot_gas: &S::Gas,
    validated_output: AuthTxOutput<S, R>,
    gas_info: GasInfo<S::Gas>,
    sequencer_da_address: &<S::Da as DaSpec>::Address,
    mut scratchpad: TxScratchpad<S, I>,
) -> (
    Result<ApplyTxResult<S>, TxProcessingError>,
    TxScratchpad<S, I>,
) {
    let (auth_tx, auth_data, message) = validated_output;

    let raw_tx_hash = auth_tx.raw_tx_hash;
    let tx = &auth_tx.authenticated_tx;

    let mut ctx = match runtime
        .transaction_authorizer()
        .resolve_unregistered_context(&auth_data, sequencer_da_address, &mut scratchpad)
    {
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

    // After this check, we are confident that the transaction sender can cover the costs of transaction processing.
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

    // The transaction will execute until one of the following conditions is met:
    // 1. It consumes more funds than `tx.max_fee`.
    // 2. The `Gas::calculate_min(tx.gas_limit, slot_gas)` is exhausted.
    let working_set_gas_meter = match tx.gas_meter(&gas_info.gas_price, slot_gas) {
        Ok(ws) => ws,
        Err(reason) => {
            return (
                Err(TxProcessingError::OutOfGas(reason.to_string())),
                scratchpad,
            )
        }
    };

    let mut working_set = WorkingSet::create_working_set(scratchpad, tx, working_set_gas_meter);

    // Here we charge the gas for the transaction sig & pre-execution checks.
    if let Err(err) = working_set.charge_gas(&gas_info.gas_used) {
        let (mut scratchpad, transaction_consumption) = working_set.revert();

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
    runtime.sequencer_remuneration().reward_sequencer_or_refund(
        sequencer_da_address,
        ctx.gas_refund_recipient(),
        sequencer_reward,
        &mut scratchpad,
    );

    (Ok(apply_tx), scratchpad)
}

#[allow(clippy::type_complexity)]
#[cfg_attr(feature = "bench", sov_modules_api::cycle_tracker)]
pub(crate) fn authenticate_unregistered_tx<S: Spec, R: Runtime<S>, I: StateProvider<S>>(
    runtime: &R,
    meter: BasicGasMeter<S>,
    batch: &BatchFromUnregisteredSequencer,
    scratchpad: TxScratchpad<S, I>,
) -> (
    Result<(AuthTxOutput<S, R>, GasInfo<S::Gas>), UnregisteredAuthenticationError>,
    TxScratchpad<S, I>,
) {
    let mut pre_exec_working_set = scratchpad.to_pre_exec_working_set(meter);

    let res = runtime.authenticate_unregistered(batch, &mut pre_exec_working_set);
    let (scratchpad, gas_meter) = pre_exec_working_set.to_scratchpad_and_gas_meter();

    match res {
        Err(e) => (Err(e), scratchpad),
        Ok(ok) => (Ok((ok, gas_meter.gas_info())), scratchpad),
    }
}

#[tracing::instrument(skip_all, name = "StfBlueprint::apply_batch")]
#[allow(clippy::too_many_arguments)]
#[cfg_attr(feature = "bench", sov_modules_api::cycle_tracker)]
pub(crate) fn apply_batch<S, RT>(
    runtime: &RT,
    mut checkpoint: StateCheckpoint<S>,
    slot_gas: &S::Gas,
    batch: BatchFromUnregisteredSequencer,
    blob_idx: usize,
    sequencer_da_address: &<S::Da as DaSpec>::Address,
    gas_price: &<S::Gas as Gas>::Price,
) -> (BatchReceipt<S>, StateCheckpoint<S>)
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

    let scratchpad = checkpoint.to_tx_scratchpad();

    debug!(
        batch_id = hex::encode(batch.id),
        "Verifying & executing transactions"
    );

    // A helper function to create a batch receipt.
    let early_return_batch_receipt =
        |tx_receipts: Vec<TransactionReceipt<S>>,
         ignored_tx_receipts: Vec<IgnoredTransactionReceipt<TxReceiptContents<S>>>,
         gas_used: <S as Spec>::Gas|
         -> BatchReceipt<S> {
            BatchReceipt {
                batch_hash: batch.id,
                tx_receipts,
                ignored_tx_receipts,
                inner: BatchSequencerReceipt {
                    da_address: sequencer_da_address.clone(),
                    gas_price: gas_price.clone(),
                    gas_used,
                    outcome: BatchSequencerOutcome {
                        rewards: Rewards {
                            accumulated_reward: 0,
                            accumulated_penalty: 0,
                        },
                    },
                },
            }
        };

    // Check: The slot gas is higher than the gas needed to validate the transaction.
    let max_unregistered_tx_check_costs = <S as GasSpec>::max_unregistered_tx_check_costs();
    if slot_gas.dim_is_less_or_eq(&max_unregistered_tx_check_costs) {
        // We don't consume gas for failed authentication of unregistered sequencer.
        let gas_used = S::Gas::zero();

        let ignored = IgnoredTransactionReceipt::<TxReceiptContents<S>> {
            ignored: IgnoredTxContents {
                gas_used: gas_used.clone(),
                index: 0,
            },
        };

        return (
            early_return_batch_receipt(Vec::new(), vec![ignored], gas_used),
            scratchpad.commit(),
        );
    }

    // We need to be cautious about potential DoS attacks. When receiving an emergency registration, we cannot immediately determine if the sequencer registration will succeed.
    // A malicious actor could exploit this mechanism to attack the rollup, for instance, by sending large transactions that are costly to deserialize from an address with no funds on the rollup.
    // To mitigate this, we initialize the gas meter with just enough gas to process a valid transaction. If the transaction is too big, we quickly run out of gas.
    // Additionally, we rate-limit (during blob selection) the number of forced registrations to further reduce the effectiveness of such attacks.
    let meter = BasicGasMeter::new_with_gas(max_unregistered_tx_check_costs, gas_price.clone());

    let authentication_result = authenticate_unregistered_tx(runtime, meter, &batch, scratchpad);

    let (validated_output, gas_info, scratchpad) = match authentication_result {
        (Ok((auth_output, gas_info)), scratchpad) => (auth_output, gas_info, scratchpad),
        // We do not consume gas for failed authentication of an unregistered sequencer, as there is no one to pay for this gas.
        // The slot gas will not decrease, but since we rate-limit the number of unregistered transactions and `max_unregistered_tx_check_costs`
        // *should* be small, we do not consider it a potential attack vector.
        (Err(UnregisteredAuthenticationError::FatalError(err, tx_hash)), scratchpad) => {
            let err_str = format!("Unregistered sequencer authentication failed: {}", err);
            warn!(error = ?err_str);

            let gas_used = S::Gas::zero();

            let skipped = SkippedTxContents {
                error: TxProcessingError::AuthenticationFailed(err_str),
                gas_used: gas_used.clone(),
            };

            return (
                early_return_batch_receipt(
                    vec![create_tx_receipt(skipped, tx_hash)],
                    Vec::new(),
                    gas_used,
                ),
                scratchpad.commit(),
            );
        }

        (Err(UnregisteredAuthenticationError::OutOfGas(reason)), scratchpad) => {
            warn!(
                error = %reason,
                "Not enough gas to authenticate the batch",
            );

            let gas_used = S::Gas::zero();

            let ignored = IgnoredTransactionReceipt::<TxReceiptContents<S>> {
                ignored: IgnoredTxContents {
                    gas_used: gas_used.clone(),
                    index: 0,
                },
            };

            return (
                early_return_batch_receipt(Vec::new(), vec![ignored], gas_used),
                scratchpad.commit(),
            );
        }
    };

    let raw_tx_hash = validated_output.0.raw_tx_hash;

    let process_tx_result = process_unauthorized_tx(
        runtime,
        slot_gas,
        validated_output,
        gas_info,
        sequencer_da_address,
        scratchpad,
    );

    let mut tx_receipts = Vec::new();
    let gas_used;
    let mut accumulated_reward = 0;

    let (tx_result, scratchpad) = process_tx_result;

    match tx_result {
        Err(error) => {
            // We don't consume gas for failed transactions.
            gas_used = S::Gas::zero();
            let skipped = SkippedTxContents {
                error,
                gas_used: gas_used.clone(),
            };

            let tx_receipt = create_tx_receipt(skipped, raw_tx_hash);
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
            gas_used = get_gas_used(&receipt);

            tx_receipts.push(receipt);
        }
    }

    let batch_receipt = BatchReceipt {
        batch_hash: batch.id,
        tx_receipts,
        ignored_tx_receipts: vec![],
        inner: BatchSequencerReceipt {
            da_address: sequencer_da_address.clone(),
            gas_price: gas_price.clone(),
            gas_used: gas_used.clone(),

            outcome: BatchSequencerOutcome {
                rewards: Rewards {
                    accumulated_reward,
                    accumulated_penalty: 0,
                },
            },
        },
    };

    checkpoint = scratchpad.commit();
    apply_batch_logs(&batch_receipt, blob_idx);

    (batch_receipt, checkpoint)
}
