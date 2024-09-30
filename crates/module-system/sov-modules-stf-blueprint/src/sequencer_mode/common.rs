#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_cycle_utils::macros::cycle_tracker;
use sov_modules_api::transaction::AuthenticatedTransactionData;
use sov_modules_api::{Context, DaSpec, DispatchCall, Error, Spec, TxScratchpad, WorkingSet};
use sov_rollup_interface::TxHash;
use tracing::info;

use crate::stf_blueprint::convert_to_runtime_events;
use crate::{
    ApplyTxResult, RevertedTxContents, Runtime, SuccessfulTxContents, TransactionReceipt, TxEffect,
};

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
