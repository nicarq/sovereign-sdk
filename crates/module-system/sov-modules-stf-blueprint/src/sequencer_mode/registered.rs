#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_cycle_utils::macros::cycle_tracker;
use sov_modules_api::capabilities::{
    AuthenticationError, AuthenticationOutput, AuthorizeSequencerError, FatalError, GasEnforcer,
    SequencerAuthorization, SequencerRemuneration, TransactionAuthenticator, TransactionAuthorizer,
    TryReserveGasError,
};
use sov_modules_api::{
    BasicGasMeter, DaSpec, ExecutionContext, FullyBakedTx, Gas, GasInfo, GasMeter,
    PreExecWorkingSet, Spec, TxScratchpad, WorkingSet,
};

use crate::sequencer_mode::common::apply_tx;
pub use crate::sequencer_mode::common::{AuthTxOutput, PreExecError};
use crate::{ApplyTxResult, Runtime, TxProcessingError};

/// Executes the entire transaction lifecycle.
#[allow(clippy::result_large_err)]
pub fn process_tx<S: Spec, D: DaSpec, R: Runtime<S, D>>(
    runtime: &R,
    auth_output: AuthenticationOutput<
        S,
        <R as TransactionAuthenticator<S>>::Decodable,
        <R as TransactionAuthenticator<S>>::AuthorizationData,
    >,
    gas_info: GasInfo<S::Gas>,
    sequencer_da_address: &D::Address,
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
                Err(TxProcessingError::CannotResolveContext(err_string)),
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
            Err(TxProcessingError::IncorrectNonce(err_string)),
            tx_scratchpad,
        );
    }

    if let Err(TryReserveGasError { reason }) =
        runtime
            .gas_enforcer()
            .try_reserve_gas(tx, &gas_info.gas_price, &ctx, &mut tx_scratchpad)
    {
        runtime.sequencer_authorization().penalize_sequencer(
            sequencer_da_address,
            &reason,
            gas_info.remaining_funds,
            &mut tx_scratchpad,
        );

        return (
            Err(TxProcessingError::CannotReserveGas(reason.to_string())),
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
                Err(TxProcessingError::OutOfGas(err.reason.to_string())),
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

/// Authenticate the transaction from the (supposedly) registered sequencer before execution
#[allow(clippy::type_complexity)]
pub fn authenticate_tx<S: Spec, Da: DaSpec, R: Runtime<S, Da>>(
    runtime: &R,
    gas_price: &<S::Gas as Gas>::Price,
    sequencer_da_address: &Da::Address,
    tx: &FullyBakedTx,
    mut scratchpad: TxScratchpad<S::Storage>,
) -> (
    Result<(AuthTxOutput<S, R>, GasInfo<S::Gas>), PreExecError>,
    TxScratchpad<S::Storage>,
) {
    // Checks the sequencer balance before the transaction is executed.
    // If the sequencer balance is not high enough, the transaction is rejected.
    let gas_meter = match runtime.sequencer_authorization().authorize_sequencer(
        sequencer_da_address,
        gas_price,
        &mut scratchpad,
    ) {
        Ok(allowed_seqiencer) => BasicGasMeter::new(allowed_seqiencer.balance, gas_price.clone()),
        Err(AuthorizeSequencerError { reason }) => {
            return (Err(PreExecError::SequencerError(reason)), scratchpad);
        }
    };

    let mut pre_exec_working_set = scratchpad.to_pre_exec_working_set(gas_meter);
    let res = authenticate_with_cycle_count(runtime, tx, &mut pre_exec_working_set);

    let gas_info = pre_exec_working_set.gas_info();
    let (mut tx_scratchpad, _) = pre_exec_working_set.to_scratchpad_and_gas_meter();
    match res {
        Err(e @ AuthenticationError::FatalError(_)) => {
            runtime
                .sequencer_remuneration()
                .slash_sequencer(sequencer_da_address, &mut tx_scratchpad);

            // Slashed
            (Err(PreExecError::AuthError(e)), tx_scratchpad)
        }
        Err(e @ AuthenticationError::OutOfGas(_)) => {
            runtime.sequencer_authorization().penalize_sequencer(
                sequencer_da_address,
                e.clone(),
                gas_info.remaining_funds,
                &mut tx_scratchpad,
            );

            (Err(PreExecError::AuthError(e)), tx_scratchpad)
        }
        Ok(ok) => (Ok((ok, gas_info)), tx_scratchpad),
    }
}

#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
fn authenticate_with_cycle_count<S: Spec, Da: DaSpec, R: Runtime<S, Da>>(
    runtime: &R,
    tx: &FullyBakedTx,
    pre_exec_working_set: &mut PreExecWorkingSet<S>,
) -> Result<AuthTxOutput<S, R>, AuthenticationError> {
    let auth_input = borsh::from_slice(&tx.data).map_err(|e| {
        AuthenticationError::FatalError(FatalError::DeserializationFailed(e.to_string()))
    })?;
    runtime.authenticate(&auth_input, pre_exec_working_set)
}
