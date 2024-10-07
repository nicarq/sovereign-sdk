#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_cycle_utils::macros::cycle_tracker;
use sov_modules_api::capabilities::{
    AuthenticationOutput, FatalError, GasEnforcer, SequencerRemuneration, TransactionAuthenticator,
    TransactionAuthorizer, TryReserveGasError, UnregisteredAuthenticationError,
};
use sov_modules_api::{
    BasicGasMeter, DaSpec, ExecutionContext, FullyBakedTx, Gas, GasInfo, GasMeter,
    PreExecWorkingSet, Spec, TxScratchpad, WorkingSet,
};

use super::registered::{AuthTxOutput, PreExecError};
use crate::sequencer_mode::common::apply_tx;
use crate::{ApplyTxResult, Runtime, TxProcessingError};

#[allow(clippy::result_large_err)]
pub fn process_unauthorized_tx<S: Spec, D: DaSpec, R: Runtime<S, D>>(
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

    if let Err(TryReserveGasError { reason }) = runtime.gas_enforcer().try_reserve_gas(
        tx,
        &gas_info.gas_price,
        ctx.sender(),
        &mut tx_scratchpad,
    ) {
        return (
            Err(TxProcessingError::CannotReserveGas(reason.to_string())),
            tx_scratchpad,
        );
    }

    let working_set: WorkingSet<S> =
        match WorkingSet::try_create_working_set(tx_scratchpad, &gas_info, tx) {
            Ok(ws) => ws,
            Err(err) => {
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

#[allow(clippy::type_complexity)]
pub(crate) fn authenticate_unregistered_tx<S: Spec, Da: DaSpec, R: Runtime<S, Da>>(
    runtime: &R,
    gas_price: &<S::Gas as Gas>::Price,
    tx: &FullyBakedTx,
    tx_scratchpad: TxScratchpad<S::Storage>,
) -> (
    Result<(AuthTxOutput<S, R>, GasInfo<S::Gas>), PreExecError>,
    TxScratchpad<S::Storage>,
) {
    // TODO #1490: Remove u64::MAX
    let meter = BasicGasMeter::new(u64::MAX, gas_price.clone());
    let mut pre_exec_working_set = tx_scratchpad.to_pre_exec_working_set(meter);

    let res = authenticate_unregistered_with_cycle_count(runtime, tx, &mut pre_exec_working_set);
    let (tx_scratchpad, gas_meter) = pre_exec_working_set.to_scratchpad_and_gas_meter();

    match res {
        Err(e) => (Err(PreExecError::UnregisteredAuthError(e)), tx_scratchpad),

        Ok(ok) => (Ok((ok, gas_meter.gas_info())), tx_scratchpad),
    }
}

#[cfg_attr(all(target_os = "zkvm", feature = "bench"), cycle_tracker)]
fn authenticate_unregistered_with_cycle_count<S: Spec, Da: DaSpec, R: Runtime<S, Da>>(
    runtime: &R,
    tx: &FullyBakedTx,
    pre_exec_working_set: &mut PreExecWorkingSet<S>,
) -> Result<AuthTxOutput<S, R>, UnregisteredAuthenticationError> {
    let auth_input = borsh::from_slice(&tx.data).map_err(|e| {
        UnregisteredAuthenticationError::FatalError(FatalError::DeserializationFailed(
            e.to_string(),
        ))
    })?;
    runtime.authenticate_unregistered(&auth_input, pre_exec_working_set)
}
