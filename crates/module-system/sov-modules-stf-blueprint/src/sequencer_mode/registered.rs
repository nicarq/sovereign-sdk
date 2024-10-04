#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_cycle_utils::macros::cycle_tracker;
use sov_modules_api::capabilities::{
    AuthenticationError, AuthenticationOutput, AuthorizeSequencerError, FatalError, GasEnforcer,
    HasCapabilities, SequencerAuthorization, SequencerRemuneration, TransactionAuthenticator,
    TransactionAuthorizer, TryReserveGasError,
};
use sov_modules_api::{
    DaSpec, ExecutionContext, FullyBakedTx, Gas, GasMeter, PreExecWorkingSet, Spec, TxScratchpad,
    WorkingSet,
};

use crate::sequencer_mode::common::apply_tx;
use crate::{ApplyTxResult, Runtime, SkippedReason, TxProcessingError};

/// Executes the entire transaction lifecycle.
#[allow(clippy::result_large_err)]
pub fn process_tx<S: Spec, D: DaSpec, R: Runtime<S, D>>(
    runtime: &R,
    raw_tx: &FullyBakedTx,
    // TODO <`https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/728`>: group constant variables in the stf-blueprint
    sequencer_da_address: &D::Address,
    gas_price: &<S::Gas as Gas>::Price,
    height: u64,
    mut scratchpad: TxScratchpad<S::Storage>,
    execution_context: ExecutionContext,
) -> (
    Result<ApplyTxResult<S>, TxProcessingError>,
    TxScratchpad<S::Storage>,
) {
    // Checks the sequencer balance before the transaction is executed.
    // If the sequencer balance is not high enough, the transaction is rejected.
    let (_, seq_stake_meter) = match runtime.sequencer_authorization().authorize_sequencer(
        sequencer_da_address,
        gas_price,
        &mut scratchpad,
    ) {
        Ok(seq_stake_meter) => seq_stake_meter,
        Err(AuthorizeSequencerError { reason }) => {
            return (
                Err(TxProcessingError::SequencerUnauthorized(reason.to_string())),
                scratchpad,
            );
        }
    };

    let mut pre_exec_working_set = scratchpad.to_pre_exec_working_set(seq_stake_meter);

    let (tx, auth_data, message) =
        match authenticate_with_cycle_count(runtime, raw_tx, &mut pre_exec_working_set) {
            Err(AuthenticationError::FatalError(reason)) => {
                let (mut tx_scratchpad, _) = pre_exec_working_set.to_scratchpad_and_gas_meter();
                runtime
                    .sequencer_remuneration()
                    .slash_sequencer(sequencer_da_address, &mut tx_scratchpad);

                // Slashed
                return (
                    Err(TxProcessingError::InvalidRegisteredTx(
                        AuthenticationError::FatalError(reason),
                    )),
                    tx_scratchpad,
                );
            }
            Err(AuthenticationError::OutOfGas(reason)) => {
                let remaining_stake = pre_exec_working_set.gas_info().remaining_funds;
                let (mut tx_scratchpad, _) = pre_exec_working_set.to_scratchpad_and_gas_meter();
                runtime.sequencer_authorization().penalize_sequencer(
                    sequencer_da_address,
                    AuthenticationError::OutOfGas(reason.clone()),
                    remaining_stake,
                    &mut tx_scratchpad,
                );

                return (
                    Err(TxProcessingError::InvalidRegisteredTx(
                        AuthenticationError::OutOfGas(reason),
                    )),
                    tx_scratchpad,
                );
            }
            Ok((tx, auth_data, message)) => (tx, auth_data, message),
        };

    let raw_tx_hash = tx.raw_tx_hash;
    let tx = &tx.authenticated_tx;

    let (mut tx_scratchpad, gas_meter) = pre_exec_working_set.to_scratchpad_and_gas_meter();
    let gas_info = gas_meter.gas_info();

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
                Err(TxProcessingError::Skipped {
                    reason: SkippedReason::CannotResolveContext(err_string),
                    raw_tx_hash,
                }),
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
            Err(TxProcessingError::Skipped {
                reason: SkippedReason::IncorrectNonce(err_string),
                raw_tx_hash,
            }),
            tx_scratchpad,
        );
    }

    if let Err(TryReserveGasError { reason }) =
        runtime
            .gas_enforcer()
            .try_reserve_gas(tx, gas_price, ctx.sender(), &mut tx_scratchpad)
    {
        runtime.sequencer_authorization().penalize_sequencer(
            sequencer_da_address,
            &reason,
            gas_info.remaining_funds,
            &mut tx_scratchpad,
        );

        return (
            Err(TxProcessingError::Skipped {
                reason: SkippedReason::CannotReserveGas(reason.to_string()),
                raw_tx_hash,
            }),
            tx_scratchpad,
        );
    }

    let working_set = match WorkingSet::try_create_working_set(tx_scratchpad, &gas_info, tx) {
        Ok(ws) => ws,
        Err(mut err) => {
            runtime.sequencer_authorization().penalize_sequencer(
                sequencer_da_address,
                &err.reason,
                gas_info.remaining_funds,
                &mut err.scratchpad,
            );

            return (
                Err(TxProcessingError::Skipped {
                    reason: SkippedReason::OutOfGas(err.reason.to_string()),
                    raw_tx_hash,
                }),
                err.scratchpad,
            );
        }
    };

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
        ctx.sequencer(),
        sequencer_reward,
        &mut tx_scratchpad,
    );

    (Ok(apply_tx), tx_scratchpad)
}

#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
fn authenticate_with_cycle_count<S: Spec, Da: DaSpec, R: Runtime<S, Da>>(
    runtime: &R,
    tx: &FullyBakedTx,
    pre_exec_working_set: &mut PreExecWorkingSet<
        S,
        <R as HasCapabilities<S, Da>>::SequencerStakeMeter,
    >,
) -> Result<
    AuthenticationOutput<
        S,
        <R as TransactionAuthenticator<S>>::Decodable,
        <R as TransactionAuthenticator<S>>::AuthorizationData,
    >,
    AuthenticationError,
> {
    let auth_input = borsh::from_slice(&tx.data).map_err(|e| {
        AuthenticationError::FatalError(FatalError::DeserializationFailed(e.to_string()))
    })?;
    runtime.authenticate(&auth_input, pre_exec_working_set)
}
