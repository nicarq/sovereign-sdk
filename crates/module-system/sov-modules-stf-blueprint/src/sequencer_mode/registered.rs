#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_cycle_utils::macros::cycle_tracker;
use sov_modules_api::capabilities::{
    AuthenticationError, AuthenticationOutput, AuthorizeSequencerError, FatalError, GasEnforcer,
    HasCapabilities, SequencerAuthorization, TransactionAuthenticator, TransactionAuthorizer,
    TryReserveGasError,
};
use sov_modules_api::{
    DaSpec, ExecutionContext, FullyBakedTx, Gas, PreExecWorkingSet, Spec, TxScratchpad,
};

use crate::sequencer_mode::common::apply_tx;
use crate::{ApplyTxResult, Runtime, TxProcessingError};

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
                return (
                    Err(TxProcessingError::AuthenticationError(
                        AuthenticationError::FatalError(reason),
                    )),
                    pre_exec_working_set.into(),
                );
            }
            Err(AuthenticationError::Invalid(reason)) => {
                // Applies the outcome of the transaction execution to update the sequencer's state.
                let tx_scratchpad = runtime.sequencer_authorization().penalize_sequencer(
                    sequencer_da_address,
                    AuthenticationError::Invalid(reason.clone()),
                    pre_exec_working_set,
                );

                return (
                    Err(TxProcessingError::AuthenticationError(
                        AuthenticationError::Invalid(reason),
                    )),
                    tx_scratchpad,
                );
            }
            Ok((tx, auth_data, message)) => (tx, auth_data, message),
        };

    let raw_tx_hash = tx.raw_tx_hash;
    let tx = &tx.authenticated_tx;

    let maybe_ctx = runtime.transaction_authorizer().resolve_context(
        &auth_data,
        sequencer_da_address,
        height,
        &mut pre_exec_working_set,
        execution_context,
    );
    let ctx = match maybe_ctx {
        Ok(ctx) => ctx,
        Err(err) => {
            let err_string = err.to_string();
            // We penalize the sequencer for the fixed amount of gas that was used to execute the transaction.
            let tx_scratchpad = runtime.sequencer_authorization().penalize_sequencer(
                sequencer_da_address,
                err,
                pre_exec_working_set,
            );

            return (
                Err(TxProcessingError::CannotResolveContext {
                    reason: err_string,
                    raw_tx_hash,
                }),
                tx_scratchpad,
            );
        }
    };

    // Check that the transaction isn't a duplicate
    if let Err(err) = runtime.transaction_authorizer().check_uniqueness(
        &auth_data,
        &ctx,
        &mut pre_exec_working_set,
    ) {
        let err_string = err.to_string();

        // We penalize the sequencer for the fixed amount of gas that was used to execute the transaction.
        let tx_scratchpad = runtime.sequencer_authorization().penalize_sequencer(
            sequencer_da_address,
            err,
            pre_exec_working_set,
        );

        return (
            Err(TxProcessingError::Nonce {
                reason: err_string,
                raw_tx_hash,
            }),
            tx_scratchpad,
        );
    }

    let working_set =
        match runtime
            .gas_enforcer()
            .try_reserve_gas(tx, ctx.sender(), pre_exec_working_set)
        {
            Ok(working_set) => working_set,
            Err(TryReserveGasError {
                reason,
                pre_exec_working_set,
            }) => {
                let reason_string = reason.to_string();
                // We penalize the sequencer for the fixed amount of gas that was used to execute the transaction.
                let tx_scratchpad = runtime.sequencer_authorization().penalize_sequencer(
                    sequencer_da_address,
                    reason,
                    pre_exec_working_set,
                );

                return (
                    Err(TxProcessingError::CannotReserveGas {
                        reason: reason_string,
                        raw_tx_hash,
                    }),
                    tx_scratchpad,
                );
            }
        };

    // If the transaction is valid, execute it and apply the changes to the state.
    let (apply_tx, tx_scratchpad) = apply_tx(
        runtime,
        ctx,
        tx,
        &auth_data,
        raw_tx_hash,
        message,
        working_set,
        sequencer_da_address,
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
