#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_cycle_utils::macros::cycle_tracker;
use sov_modules_api::capabilities::{AuthenticationError, AuthenticationOutput};
use sov_modules_api::transaction::AuthenticatedTransactionData;
use sov_modules_api::{
    BatchSequencerReceipt, Context, DaSpec, DispatchCall, Error, Gas, Spec, TxScratchpad,
    WorkingSet,
};
use sov_rollup_interface::TxHash;
use tracing::{debug, info, warn};

use crate::stf_blueprint::convert_to_runtime_events;
use crate::{
    ApplyTxResult, RevertedTxContents, Runtime, SuccessfulTxContents, TransactionAuthenticator,
    TxEffect, TxProcessingError, TxReceiptContents,
};

/// The receipt type for a transacition using the STF blueprint.
pub type TransactionReceipt<S> =
    sov_rollup_interface::stf::TransactionReceipt<TxReceiptContents<S>>;

/// Applies a single transaction to the current state. In normal execution, we commit twice times execution:
/// 1. After the pre-dispatch hook. This ensures that the gas charges are paid even if the transaction fails later during execution
/// 2. After the post-dispatch hook. This ensures that the transaction can be reverted by the post-dispatch hook if desired.
#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_tx<S, RT, Da>(
    runtime: &RT,
    ctx: &Context<S>,
    tx: &AuthenticatedTransactionData<S>,
    raw_tx_hash: TxHash,
    message: <RT as DispatchCall>::Decodable,
    mut working_set: WorkingSet<S>,
) -> (ApplyTxResult<S>, TxScratchpad<S::Storage>)
where
    S: Spec,
    Da: DaSpec,
    RT: Runtime<S, Da>,
{
    let tx_result = attempt_tx(tx, message, ctx, runtime, &mut working_set);
    let (tx_scratchpad, receipt, transaction_consumption) = match tx_result {
        Ok(_) => {
            let (tx_scratchpad, transaction_consumption, events) = working_set.finalize();
            let gas_used = transaction_consumption.base_fee();

            (
                tx_scratchpad,
                TransactionReceipt {
                    tx_hash: raw_tx_hash,
                    body_to_save: None,
                    events: convert_to_runtime_events::<S, RT, Da>(events),
                    receipt: TxEffect::Successful(SuccessfulTxContents {
                        gas_used: gas_used.clone(),
                    }),
                },
                transaction_consumption,
            )
        }
        Err(error) => {
            // It's expected that transactions will revert, so we log them at the info level.
            info!(
                %error,
                %raw_tx_hash,
                "Tx was reverted",
            );
            // the transaction causing invalid state transition is reverted,
            // but we don't slash and continue processing remaining transactions.
            // working_set.revert_in_place();
            let (tx_scratchpad, transaction_consumption) = working_set.revert();

            let receipt = TransactionReceipt {
                tx_hash: raw_tx_hash,
                body_to_save: None,
                events: vec![], // As in Ethereum, reverted transactions don't emit events
                receipt: TxEffect::Reverted(RevertedTxContents {
                    gas_used: transaction_consumption.base_fee().clone(),
                    reason: error,
                }),
            };

            (tx_scratchpad, receipt, transaction_consumption)
        }
    };

    (
        ApplyTxResult::<S> {
            transaction_consumption,
            receipt,
        },
        tx_scratchpad,
    )
}

fn attempt_tx<S: Spec, Da: DaSpec, RT: Runtime<S, Da>>(
    tx: &AuthenticatedTransactionData<S>,
    message: <RT as DispatchCall>::Decodable,
    ctx: &Context<S>,
    runtime: &RT,
    state: &mut WorkingSet<S>,
) -> Result<(), Error> {
    runtime.pre_dispatch_tx_hook(tx, state)?;

    runtime.dispatch_call(message, state, ctx)?;

    runtime.post_dispatch_tx_hook(tx, ctx, state)?;

    Ok(())
}

/// Error during the pre-flight checks before the transaction is executed.
#[derive(Debug, thiserror::Error)]
pub enum PreExecError {
    /// Sequencer error.
    #[error("Sequencer error")]
    SequencerError(#[source] anyhow::Error),
    /// Invalid transaction from registered sequencer.
    #[error("Invalid transaction from registered sequencer")]
    AuthError(#[source] AuthenticationError),
}

/// Alias for `AuthenticationOutput`.
pub type AuthTxOutput<S, R> = AuthenticationOutput<
    S,
    <R as TransactionAuthenticator<S>>::Decodable,
    <R as TransactionAuthenticator<S>>::AuthorizationData,
>;

/// The receipt for a batch using the STF blueprint.
pub type BatchReceipt<S, Da> = sov_rollup_interface::stf::BatchReceipt<
    BatchSequencerReceipt<Da>,
    TxReceiptContents<S>,
    <<S as Spec>::Gas as Gas>::Price,
>;

pub(crate) const BEGIN_BATCH_HOOK_ERR: &str = "Error: The batch was rejected by the 'begin_batch_hook' hook. Skipping batch without slashing the sequencer";

/// Returns the gas used by a transaction from its receipt.
pub fn get_gas_used<S: Spec>(receipt: &TransactionReceipt<S>) -> S::Gas {
    match receipt.receipt {
        TxEffect::Successful(ref successful) => successful.gas_used.clone(),
        TxEffect::Reverted(ref reverted) => reverted.gas_used.clone(),
        TxEffect::Skipped(_) => S::Gas::zero(),
    }
}

pub(crate) fn create_tx_receipt<S: Spec>(
    reason: TxProcessingError,
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

pub(crate) fn apply_batch_logs<S: Spec, Da: DaSpec>(
    batch_receipt: &BatchReceipt<S, Da>,
    gas_used: &S::Gas,
    blob_idx: usize,
) {
    let batch_sequencer_receipt = &batch_receipt.inner;

    info!(
        blob_idx,
        blob_hash = hex::encode(batch_receipt.batch_hash),
        sequencer_da_address = %batch_sequencer_receipt.da_address,
        num_txs = batch_receipt.tx_receipts.len(),
        sequencer_outcome = ?batch_receipt.inner,
        ?gas_used,
        "Applied blob and got the sequencer outcome"
    );

    info!(sequencer_da_address =
        ?batch_sequencer_receipt.da_address, ?batch_sequencer_receipt.outcome, "BatchSequencerOutcome ");

    for (i, tx_receipt) in batch_receipt.tx_receipts.iter().enumerate() {
        debug!(
            tx_idx = i,
            tx_hash = hex::encode(tx_receipt.tx_hash),
            receipt = ?tx_receipt.receipt,
            gas_used = ?get_gas_used(tx_receipt),
            "Tx receipt"
        );
    }
}
