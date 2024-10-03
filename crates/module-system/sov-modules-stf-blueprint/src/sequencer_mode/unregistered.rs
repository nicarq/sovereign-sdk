#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_cycle_utils::macros::cycle_tracker;
use sov_modules_api::capabilities::{
    AuthenticationOutput, FatalError, GasEnforcer, SequencerRemuneration, TransactionAuthenticator,
    TransactionAuthorizer, TryReserveGasError, UnregisteredAuthenticationError,
};
use sov_modules_api::{
    DaSpec, ExecutionContext, FullyBakedTx, Gas, GasMeter, GasSpec, PreExecWorkingSet, Spec,
    TxScratchpad, UnlimitedGasMeter, WorkingSet,
};

use crate::sequencer_mode::common::apply_tx;
use crate::{ApplyTxResult, Runtime, SkippedReason, TxProcessingError};

#[allow(clippy::result_large_err)]
pub fn process_unauthorized_tx<S: Spec, D: DaSpec, R: Runtime<S, D>>(
    runtime: &R,
    raw_tx: &FullyBakedTx,
    sequencer_da_address: &D::Address,
    gas_price: &<S::Gas as Gas>::Price,
    height: u64,
    tx_scratchpad: TxScratchpad<S::Storage>,
    execution_context: ExecutionContext,
) -> (
    Result<ApplyTxResult<S>, TxProcessingError>,
    TxScratchpad<S::Storage>,
) {
    let mut pre_exec_working_set =
        tx_scratchpad.to_pre_exec_working_set(UnlimitedGasMeter::new_with_price(gas_price.clone()));

    let (tx, auth_data, message) = match authenticate_unregistered_with_cycle_count(
        runtime,
        raw_tx,
        &mut pre_exec_working_set,
    ) {
        Ok(v) => v,
        Err(e) => {
            return (
                Err(TxProcessingError::InvalidUnregisteredTx(e)),
                pre_exec_working_set.to_scratchpad_and_gas_meter().0,
            );
        }
    };

    let raw_tx_hash = tx.raw_tx_hash;
    let tx = &tx.authenticated_tx;

    let (mut tx_scratchpad, mut gas_meter) = pre_exec_working_set.to_scratchpad_and_gas_meter();

    let ctx = match runtime
        .transaction_authorizer()
        .resolve_unregistered_context(&auth_data, height, &mut tx_scratchpad, execution_context)
    {
        Ok(ctx) => ctx,
        Err(e) => {
            return (
                Err(TxProcessingError::Skipped {
                    reason: SkippedReason::CannotResolveContext(e.to_string()),
                    raw_tx_hash,
                }),
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
            Err(TxProcessingError::Skipped {
                reason: SkippedReason::IncorrectNonce(e.to_string()),
                raw_tx_hash,
            }),
            tx_scratchpad,
        );
    }

    if let Err(e) = gas_meter.charge_gas(&S::gas_forced_sequencer_registration_cost()) {
        return (
            Err(TxProcessingError::Skipped {
                reason: SkippedReason::CannotReserveGas(e.to_string()),
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
        return (
            Err(TxProcessingError::Skipped {
                reason: SkippedReason::CannotReserveGas(reason.to_string()),
                raw_tx_hash,
            }),
            tx_scratchpad,
        );
    }

    let working_set: WorkingSet<S> =
        match WorkingSet::try_create_working_set(tx_scratchpad, &gas_meter, tx) {
            Ok(ws) => ws,
            Err(err) => {
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
fn authenticate_unregistered_with_cycle_count<S: Spec, Da: DaSpec, R: Runtime<S, Da>>(
    runtime: &R,
    tx: &FullyBakedTx,
    pre_exec_working_set: &mut PreExecWorkingSet<S, UnlimitedGasMeter<S::Gas>>,
) -> Result<
    AuthenticationOutput<
        S,
        <R as TransactionAuthenticator<S>>::Decodable,
        <R as TransactionAuthenticator<S>>::AuthorizationData,
    >,
    UnregisteredAuthenticationError,
> {
    let auth_input = borsh::from_slice(&tx.data).map_err(|e| {
        UnregisteredAuthenticationError::FatalError(FatalError::DeserializationFailed(
            e.to_string(),
        ))
    })?;
    runtime.authenticate_unregistered(&auth_input, pre_exec_working_set)
}
