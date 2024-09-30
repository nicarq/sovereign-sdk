#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_cycle_utils::macros::cycle_tracker;
use sov_modules_api::capabilities::AuthenticationError;
use sov_modules_api::runtime::capabilities::KernelSlotHooks;
use sov_modules_api::transaction::SequencerReward;
use sov_modules_api::{
    BatchSequencerOutcome, BatchSequencerReceipt, BatchWithId, DaSpec, ExecutionContext, Gas,
    GasArray, Spec, StateCheckpoint,
};
use sov_rollup_interface::TxHash;
use tracing::{debug, error, warn};

use crate::sequencer_mode::unregistered::process_unauthorized_tx;
use crate::{
    process_tx, ApplyTxResult, Runtime, SkippedReason, TxEffect, TxProcessingError,
    TxReceiptContents,
};

/// The receipt type for a transacition using the STF blueprint.
pub type TransactionReceipt<S> =
    sov_rollup_interface::stf::TransactionReceipt<TxReceiptContents<S>>;

/// The receipt for a batch using the STF blueprint.
pub type BatchReceipt<S, Da> = sov_rollup_interface::stf::BatchReceipt<
    BatchSequencerReceipt<Da>,
    TxReceiptContents<S>,
    <<S as Spec>::Gas as Gas>::Price,
>;

const BEGIN_BATCH_HOOK_ERR: &str = "Error: The batch was rejected by the 'begin_batch_hook' hook. Skipping batch without slashing the sequencer";

#[tracing::instrument(skip_all, name = "StfBlueprint::apply_batch")]
#[allow(clippy::too_many_arguments)]
#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
pub(crate) fn apply_batch<S, Da, RT, K>(
    runtime: &RT,
    mut checkpoint: StateCheckpoint<S::Storage>,
    batch_with_id: BatchWithId,
    sequencer_da_address: Da::Address,
    gas_price: &<S::Gas as Gas>::Price,
    height: u64,
    is_registered_sequencer: bool,
    execution_context: ExecutionContext,
) -> (BatchReceipt<S, Da>, StateCheckpoint<S::Storage>, S::Gas)
where
    S: Spec,
    Da: DaSpec,
    RT: Runtime<S, Da>,
    K: KernelSlotHooks<S, Da>,
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
        let process_tx_result = if is_registered_sequencer {
            process_tx(
                runtime,
                raw_tx,
                &sequencer_da_address,
                gas_price,
                height,
                tx_scratchpad,
                execution_context,
            )
        } else {
            process_unauthorized_tx(
                runtime,
                raw_tx,
                &sequencer_da_address,
                gas_price,
                height,
                tx_scratchpad,
                execution_context,
            )
        };

        let (tx_result, tx_scratchpad) = process_tx_result;
        checkpoint = tx_scratchpad.commit();
        match tx_result {
            Err(reason) => {
                match reason {
                    TxProcessingError::SequencerUnauthorized(reason) => {
                        let remaining = raw_txs.len() - idx - 1;
                        error!(
                            reason = %reason,
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
                            checkpoint,
                            gas_used,
                        );
                    }

                    TxProcessingError::InvalidUnregisteredTx(reason) => {
                        warn!(
                            sequencer_da_address = %sequencer_da_address,
                            reason = %reason,
                            "Processing of unregistered sequencer transaction raised error, skipping"
                        );

                        return (
                            BatchReceipt {
                                batch_hash: batch_with_id.id,
                                tx_receipts: Vec::new(),
                                inner: BatchSequencerReceipt {
                                    da_address: sequencer_da_address,
                                    outcome: BatchSequencerOutcome::Ignored(reason.to_string()),
                                },
                                gas_price: gas_price.clone(),
                            },
                            checkpoint,
                            gas_used,
                        );
                    }

                    err @ TxProcessingError::InvalidRegisteredTx(AuthenticationError::OutOfGas(
                        _,
                    )) => {
                        // TODO: https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/901
                        error!(error = ?err, "Transaction will be completely forgotten, just like tears in the rain.");
                    }
                    // If the sequencer raised a fatal error then he needs to get slashed and we stop applying the batch
                    TxProcessingError::InvalidRegisteredTx(AuthenticationError::FatalError(
                        err,
                    )) => {
                        error!(
                                sequencer_da_address = %sequencer_da_address,
                                err=%err, "Tx authentication raised a fatal error, sequencer slashed");

                        return (
                            BatchReceipt {
                                batch_hash: batch_with_id.id,
                                tx_receipts,
                                inner: BatchSequencerReceipt {
                                    da_address: sequencer_da_address,
                                    outcome: BatchSequencerOutcome::Slashed(err),
                                },
                                gas_price: gas_price.clone(),
                            },
                            checkpoint,
                            gas_used,
                        );
                    }

                    TxProcessingError::Skipped {
                        reason,
                        raw_tx_hash,
                    } => {
                        let tx_receipt = create_tx_receipt(reason, raw_tx_hash, idx);
                        tx_receipts.push(tx_receipt);
                    }
                }
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

    (
        BatchReceipt {
            batch_hash: batch_with_id.id,
            tx_receipts,
            inner: BatchSequencerReceipt {
                da_address: sequencer_da_address,
                outcome: BatchSequencerOutcome::Rewarded(accumulated_reward),
            },
            gas_price: gas_price.clone(),
        },
        checkpoint,
        gas_used,
    )
}

/// Returns the gas used by a transaction from its receipt.
pub fn get_gas_used<S: Spec>(receipt: &TransactionReceipt<S>) -> S::Gas {
    match receipt.receipt {
        TxEffect::Successful(ref successful) => successful.gas_used.clone(),
        TxEffect::Reverted(ref reverted) => reverted.gas_used.clone(),
        TxEffect::Skipped(_) => S::Gas::zero(),
    }
}

fn create_tx_receipt<S: Spec>(
    reason: SkippedReason,
    raw_tx_hash: TxHash,
    idx: usize,
) -> TransactionReceipt<S> {
    warn!(
        error = %reason,
        raw_tx_hash = %raw_tx_hash,
        tx_idx = %idx,
        "An error occurred while processing a transaction. The transaction was not executed. The sequencer was penalized.",
    );

    TransactionReceipt {
        tx_hash: raw_tx_hash,
        body_to_save: None,
        events: Vec::new(),
        receipt: TxEffect::Skipped(reason),
    }
}
