use sov_modules_api::capabilities::{
    AuthenticationError, GasEnforcer, SequencerAuthorization, SequencerRemuneration,
    TransactionAuthorizer, TryReserveGasError,
};
use sov_modules_api::transaction::TransactionConsumption;
#[cfg(feature = "native")]
use sov_modules_api::NestedEnumUtils;
use sov_modules_api::{
    BasicGasMeter, BatchSequencerOutcome, BatchSequencerReceipt, DaSpec, ExecutionContext,
    FullyBakedTx, Gas, GasArray, GasMeter, GasSpec, IgnoredTransactionReceipt, IncrementalBatch,
    InjectedControlFlow, PreExecWorkingSet, ProvisionalSequencerOutcome, Rewards, SlotGasMeter,
    Spec, StateCheckpoint, StateProvider, TxControlFlow, TxScratchpad, WorkingSet,
};
use sov_rollup_interface::TxHash;
use tracing::{trace, warn};

pub use crate::sequencer_mode::common::PreExecError;
use crate::sequencer_mode::common::{
    apply_batch_logs, apply_tx, create_tx_receipt, get_gas_used, BatchReceipt,
};
use crate::{
    ApplyTxResult, AuthTxOutput, IgnoredTxContents, Runtime, SkippedTxContents, TransactionReceipt,
    TxProcessingError, TxReceiptContents,
};

/// Executes the entire transaction lifecycle.
#[allow(clippy::result_large_err, clippy::too_many_arguments)]
#[cfg_attr(feature = "native", tracing::instrument(skip_all, name = "StfBlueprint::process_tx", fields(context = ?execution_context)))]
#[cfg_attr(feature = "bench", sov_modules_api::cycle_tracker)]
pub fn process_tx<S, R, I, C>(
    runtime: &R,
    pre_exec_gas_meter: &BasicGasMeter<S>,
    slot_gas: &S::Gas,
    validated_output: AuthTxOutput<S, R>,
    sequencer_da_address: &<S::Da as DaSpec>::Address,
    scratchpad: TxScratchpad<S, I>,
    #[allow(unused_variables)] execution_context: ExecutionContext,
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
    let visible_slot_number =
        sov_modules_api::VersionReader::visible_slot_number_to_access(&scratchpad);

    #[cfg(feature = "native")]
    let (start, discriminant) = {
        (
            std::time::Instant::now(),
            format!("{:?}", validated_output.2.discriminant()),
        )
    };

    let result = process_tx_inner(
        runtime,
        pre_exec_gas_meter,
        slot_gas,
        validated_output,
        sequencer_da_address,
        scratchpad,
        injected_control_flow,
    );

    #[cfg(feature = "native")]
    track_transaction_metrics(
        &result.0,
        start.elapsed(),
        execution_context,
        visible_slot_number,
        sequencer_da_address,
        discriminant,
    );

    result
}

#[cfg(feature = "native")]
fn track_transaction_metrics<S: Spec>(
    result: &Result<ApplyTxResult<S>, TxProcessingError>,
    execution_time: std::time::Duration,
    execution_context: ExecutionContext,
    visible_slot_number: sov_rollup_interface::common::VisibleSlotNumber,
    sequencer_address: &<S::Da as DaSpec>::Address,
    message_discriminant: String,
) {
    sov_metrics::track_metrics(|metrics_tracker| {
        let tx_effect = match result {
            Ok(tx_result) => sov_metrics::TransactionEffect::from(&tx_result.receipt.receipt),
            Err(_) => sov_metrics::TransactionEffect::Skipped,
        };

        let transaction_metrics = sov_metrics::TransactionProcessingMetrics {
            execution_time,
            tx_effect,
            execution_context,
            visible_slot_number,
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
    pre_exec_gas_meter: &BasicGasMeter<S>,
    slot_gas: &S::Gas,
    validated_output: AuthTxOutput<S, R>,
    sequencer_da_address: &<S::Da as DaSpec>::Address,
    mut scratchpad: TxScratchpad<S, I>,
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
    let (auth_tx, auth_data, message) = validated_output;

    let raw_tx_hash = auth_tx.raw_tx_hash;
    let tx = &auth_tx.authenticated_tx;

    let maybe_ctx = runtime.transaction_authorizer().resolve_context(
        &auth_data,
        sequencer_da_address,
        &mut scratchpad,
    );
    let mut ctx = match maybe_ctx {
        Ok(ctx) => ctx,
        Err(err) => {
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
        return (
            Err(TxProcessingError::IncorrectNonce(err.to_string())),
            scratchpad,
        );
    }

    if let Err(TryReserveGasError { reason }) = runtime.gas_enforcer().try_reserve_gas(
        tx,
        &pre_exec_gas_meter.gas_info().gas_price,
        &mut ctx,
        &mut scratchpad,
    ) {
        return (
            Err(TxProcessingError::CannotReserveGas(reason.to_string())),
            scratchpad,
        );
    }

    let working_set_gas_meter =
       // The transaction will execute until one of the following conditions is met:
       // 1. It consumes more funds than `tx.max_fee`.
       // 2. The `Gas::calculate_min(tx.gas_limit, slot_gas)` is exhausted.
        match tx.gas_meter(&pre_exec_gas_meter.gas_info().gas_price, slot_gas) {
            Ok(ws) => ws,
            Err(reason) => {
                return (
                    Err(TxProcessingError::OutOfGas(reason.to_string())),
                    scratchpad,
                )
            }
        };

    let mut working_set = WorkingSet::create_working_set(scratchpad, tx, working_set_gas_meter);

    // Recover the authentication cost form the user.
    if let Err(err) = working_set.charge_gas(&pre_exec_gas_meter.gas_info().gas_used) {
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
    runtime.sequencer_remuneration().reward_sequencer(
        sequencer_da_address,
        sequencer_reward,
        &mut scratchpad,
    );

    (Ok(apply_tx), scratchpad)
}

#[cfg_attr(feature = "bench", sov_modules_api::cycle_tracker)]
#[cfg_attr(
    feature = "native",
    tracing::instrument(skip_all, name = "StfBlueprint::authenticate")
)]
fn deserialize_and_authenticate<S: Spec, R: Runtime<S>, I: StateProvider<S>>(
    runtime: &R,
    tx: &FullyBakedTx,
    pre_exec_working_set: &mut PreExecWorkingSet<S, I>,
) -> Result<AuthTxOutput<S, R>, AuthenticationError> {
    runtime.authenticate(tx, pre_exec_working_set)
}

pub struct IncrementalBatchReceipt<S: Spec> {
    pub tx_receipts: Vec<TransactionReceipt<S>>,
    pub ignored_tx_receipts: Vec<IgnoredTransactionReceipt<TxReceiptContents<S>>>,
    pub inner: BatchSequencerReceipt<S>,
}

impl<S: Spec> IncrementalBatchReceipt<S> {
    pub fn finalize(self, id: [u8; 32]) -> BatchReceipt<S> {
        BatchReceipt {
            batch_hash: id,
            tx_receipts: self.tx_receipts,
            ignored_tx_receipts: self.ignored_tx_receipts,
            inner: self.inner,
        }
    }
}

#[tracing::instrument(skip_all, name = "StfBlueprint::apply_batch", fields(context = ?execution_context))]
#[allow(clippy::too_many_arguments)]
#[cfg_attr(feature = "bench", sov_modules_api::cycle_tracker)]
pub(crate) fn apply_batch<S, RT, B>(
    runtime: &RT,
    mut checkpoint: StateCheckpoint<S>,
    slot_gas_meter: &mut SlotGasMeter<S>,
    mut batch_with_id: B,
    blob_idx: usize,
    sequencer_da_address: &<S::Da as DaSpec>::Address,
    gas_price: &<S::Gas as Gas>::Price,
    execution_context: ExecutionContext,
) -> (IncrementalBatchReceipt<S>, StateCheckpoint<S>)
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

    trace!(
        sequencer_da_address = %sequencer_da_address,
        ?gas_price,
        "Applying a batch"
    );

    batch_with_id.pre_flight(&checkpoint);
    let mut clean_scratchpad = checkpoint.to_tx_scratchpad();

    trace!("Verifying & executing transactions");

    // Cost of the authentication for the entire batch.
    // It should include the costs of `authentication` and process_tx pre-execution checks.
    if !execution_context.is_sequencer() {
        assert!(
            batch_with_id.known_remaining_txs().is_some(),
            "Batch sizes are always known by the time the batch appears on the DA Layer"
        );
    }

    let mut tx_receipts = Vec::with_capacity(batch_with_id.known_remaining_txs().unwrap_or(128));
    let mut ignored_tx_receipts = Vec::default();
    let mut accumulated_reward = 0;
    let mut accumulated_penalty = 0;

    for (idx, (raw_tx, injected_control_flow)) in batch_with_id.enumerate() {
        let sequencer = runtime
            .sequencer_authorization()
            .authorize_sequencer(sequencer_da_address, &mut clean_scratchpad)
            .expect("Blob selection must guarantee that sequencer is registered");

        let sequencer_bond = sequencer.balance;

        let AuthAndProcessOutput {
            gas_used,
            scratchpad: dirty_scratchpad,
            outcome,
        } = auth_and_process_tx(
            runtime,
            clean_scratchpad,
            // Here we make sure that a tx can't use more gas that remaining gas in the slot gas meter.
            slot_gas_meter.remaining_slot_gas(sequencer_da_address),
            &raw_tx,
            sequencer_da_address,
            gas_price,
            execution_context,
            sequencer_bond,
            idx,
            &injected_control_flow,
        );

        let provisional_outcome = match outcome {
            AuthAndProcessOutcome::IllegalSequencer { reason } => {
                tracing::warn!("Transaction could not be attempted due to sequencer error. If this error persists, check that your sequencer has sufficient funds. Error: {}", reason);
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
                // SAFETY: It is safe to unwrap here because the total gas used is guaranteed to be less than the slot gas limit.
                slot_gas_meter
                    .charge_gas(&gas_used, sequencer_da_address)
                    .expect("Gas Overflow");

                accumulated_reward += provisional_reward;
                accumulated_penalty += provisional_penalty;
                tx_receipts.push(receipt);
            }
            TxControlFlow::IgnoreTx => {
                // We don't actually `ignore` transactions unless we're just provisionally executing in the sequencer.
                if !execution_context.is_sequencer() {
                    // SAFETY: It is safe to unwrap here because the total gas used is guaranteed to be less than the slot gas limit.
                    slot_gas_meter
                        .charge_gas(&gas_used, sequencer_da_address)
                        .expect("Gas Overflow");

                    accumulated_reward += provisional_reward;
                    accumulated_penalty += provisional_penalty;

                    // In onchain mode, transactions that make the sequencer run out of gas are treated as "ignored".
                    // While they consume gas, their hashes cannot be computed, so they are not indexed in the database.
                    let ignored = IgnoredTransactionReceipt::<TxReceiptContents<S>> {
                        ignored: IgnoredTxContents {
                            gas_used,
                            index: idx,
                        },
                    };

                    ignored_tx_receipts.push(ignored);
                }
            }
        }
        clean_scratchpad = new_checkpoint.to_tx_scratchpad();
    }
    let total_gas_used_in_batch = slot_gas_meter.total_gas_used();

    // End of the transaction processing phase.
    let batch_receipt = IncrementalBatchReceipt {
        tx_receipts,
        ignored_tx_receipts,
        inner: BatchSequencerReceipt {
            da_address: sequencer_da_address.clone(),
            gas_price: gas_price.clone(),
            gas_used: total_gas_used_in_batch.clone(),
            outcome: BatchSequencerOutcome {
                rewards: Rewards {
                    accumulated_reward,
                    accumulated_penalty,
                },
            },
        },
    };

    checkpoint = clean_scratchpad.commit();
    apply_batch_logs(&batch_receipt, blob_idx);
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

fn penalize_sequencer<S: Spec, RT: Runtime<S>, I: StateProvider<S>>(
    runtime: &RT,
    auth_cost: u64,
    sequencer_da_address: &<S::Da as DaSpec>::Address,
    tx_scratchpad: &mut TxScratchpad<S, I>,
) {
    runtime
        .gas_enforcer()
        .transfer_funds_from_sequencer_to_prover(auth_cost, sequencer_da_address, tx_scratchpad)
        // We ensure that the sequencer bond is at least `max_tx_check_value` so this should never fail.
        .expect("Sequencer should have enough funds to pay for the pre-execution checks");
}

#[cfg_attr(feature = "bench", sov_modules_api::cycle_tracker)]
#[allow(clippy::too_many_arguments)]
fn auth_and_process_tx<S, RT, I, C>(
    runtime: &RT,
    mut scratchpad: TxScratchpad<S, I>,
    slot_gas: &S::Gas,
    raw_tx: &FullyBakedTx,
    sequencer_da_address: &<S::Da as DaSpec>::Address,
    gas_price: &<S::Gas as Gas>::Price,
    execution_context: ExecutionContext,
    sequencer_bond: u64,
    idx: usize,
    injected_control_flow: &C,
) -> AuthAndProcessOutput<S, I>
where
    S: Spec,
    RT: Runtime<S>,
    I: StateProvider<S>,
    C: InjectedControlFlow<TransactionReceipt<S>, S>,
{
    // CHECKS:
    // 1. `max_tx_check_costs` will not cause an overflow when converted to a token value.
    let max_tx_check_costs = <S as GasSpec>::max_tx_check_costs();
    let max_tx_check_value = match max_tx_check_costs.checked_value(gas_price) {
        Some(v) => v,
        None => {
            return AuthAndProcessOutput {
                outcome: AuthAndProcessOutcome::IllegalSequencer {
                    reason: "Overflow: Unable to calculate gas value for max_tx_check_costs"
                        .to_string(),
                },
                scratchpad,
                gas_used: <S as Spec>::Gas::zero(),
            }
        }
    };

    // 2. The sequencer has more funds than are needed for transaction validation..
    if sequencer_bond <= max_tx_check_value {
        return AuthAndProcessOutput {
            outcome: AuthAndProcessOutcome::IllegalSequencer {
                reason: format!(
                    "The sequencer did not have sufficient funds to cover tx authentication checks, sequencer bond is {}, but the cost of checking the transaction is {}",
                    sequencer_bond,
                    max_tx_check_value
                ),
            },
            scratchpad,
            gas_used: <S as Spec>::Gas::zero(),
        };
    }

    // 3. The slot gas is higher than the gas needed to validate the transaction.
    if slot_gas.dim_is_less_or_eq(&max_tx_check_costs) {
        return AuthAndProcessOutput {
            outcome: AuthAndProcessOutcome::IllegalSequencer {
                reason: "The slot gas limit has been exhausted".to_string(),
            },
            scratchpad,
            gas_used: <S as Spec>::Gas::zero(),
        };
    }

    // In the conditions above, we ensured that both the sequencer bond and the remaining gas in the slot gas meter exceed `max_tx_check_costs`.
    // Initialize `pre_exec_gas_meter` with `max_tx_check_costs` gas.
    let pre_exec_gas_meter = BasicGasMeter::new_with_gas(max_tx_check_costs, gas_price.clone());

    let mut pre_exec_working_set: PreExecWorkingSet<S, _> =
        scratchpad.to_pre_exec_working_set(pre_exec_gas_meter);

    // Charge gas for all the checks in the `process_tx`.
    // SAFETY: We can unwrap here because, we asserted that max_tx_check_costs > process_tx_pre_exec_checks_gas.
    pre_exec_working_set
        .charge_gas(&<S as GasSpec>::process_tx_pre_exec_checks_gas())
        .expect("The gas meter should be able to charge the pre-execution checks");

    let authentication_result =
        deserialize_and_authenticate(runtime, raw_tx, &mut pre_exec_working_set);

    let (next_scratchpad, returned_meter) = pre_exec_working_set.to_scratchpad_and_gas_meter();
    let pre_exec_gas_meter = returned_meter;

    scratchpad = next_scratchpad;

    let validated_output = match authentication_result {
        Ok(auth_output) => auth_output,
        Err(pre_exec_error) => match pre_exec_error {
            AuthenticationError::FatalError(err, tx_hash) => {
                penalize_sequencer(
                    runtime,
                    pre_exec_gas_meter.gas_info().gas_value,
                    sequencer_da_address,
                    &mut scratchpad,
                );

                let gas_used_for_authentication = pre_exec_gas_meter.gas_info().gas_used;
                return AuthAndProcessOutput {
                    scratchpad,
                    gas_used: gas_used_for_authentication,
                    outcome: AuthAndProcessOutcome::Skipped {
                        error: TxProcessingError::AuthenticationFailed(err.to_string()),
                        tx_hash,
                    },
                };
            }
            AuthenticationError::OutOfGas(e) => {
                penalize_sequencer(
                    runtime,
                    pre_exec_gas_meter.gas_info().gas_value,
                    sequencer_da_address,
                    &mut scratchpad,
                );

                let gas_used_for_authentication = pre_exec_gas_meter.gas_info().gas_used;
                return AuthAndProcessOutput {
                        scratchpad,
                        gas_used: gas_used_for_authentication,
                        outcome: AuthAndProcessOutcome::IllegalSequencer {
                            reason: format!("The sequencer did not have sufficient funds to cover tx authentication: {}", e),
                        },
                    };
            }
        },
    };

    // Begin the transaction processing phase.
    let raw_tx_hash = validated_output.0.raw_tx_hash;
    let span = tracing::info_span!("transaction", id = %raw_tx_hash, idx = %idx).entered();

    let process_tx_result = process_tx(
        runtime,
        &pre_exec_gas_meter,
        slot_gas,
        validated_output,
        sequencer_da_address,
        scratchpad,
        execution_context,
        injected_control_flow,
    );

    span.exit();

    let (tx_result, mut scratchpad) = process_tx_result;

    match tx_result {
        Err(error) => {
            penalize_sequencer(
                runtime,
                pre_exec_gas_meter.gas_info().gas_value,
                sequencer_da_address,
                &mut scratchpad,
            );

            let gas_used = pre_exec_gas_meter.gas_info().gas_used;
            AuthAndProcessOutput {
                outcome: AuthAndProcessOutcome::Skipped {
                    error,
                    tx_hash: raw_tx_hash,
                },
                scratchpad,
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
                scratchpad,
                outcome: AuthAndProcessOutcome::Applied {
                    receipt,
                    transaction_consumption,
                },
            }
        }
    }
}
