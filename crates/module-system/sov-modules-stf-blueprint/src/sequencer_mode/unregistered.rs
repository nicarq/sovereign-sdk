#[cfg(all(target_os = "zkvm", feature = "bench"))]
use sov_cycle_utils::macros::cycle_tracker;
use sov_modules_api::capabilities::{
    AuthenticationOutput, FatalError, GasEnforcer, TransactionAuthenticator, TransactionAuthorizer,
    TryReserveGasError, UnregisteredAuthenticationError,
};
use sov_modules_api::{
    DaSpec, ExecutionContext, FullyBakedTx, Gas, GasMeter, GasSpec, PreExecWorkingSet, Spec,
    TxScratchpad, UnlimitedGasMeter,
};

use crate::sequencer_mode::common::apply_tx;
use crate::{ApplyTxResult, Runtime, TxProcessingError};

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
                Err(TxProcessingError::InvalidUnregisteredTx(e.to_string())),
                pre_exec_working_set.into(),
            );
        }
    };

    let raw_tx_hash = tx.raw_tx_hash;
    let tx = &tx.authenticated_tx;

    let ctx = match runtime
        .transaction_authorizer()
        .resolve_unregistered_context(
            &auth_data,
            height,
            &mut pre_exec_working_set,
            execution_context,
        ) {
        Ok(ctx) => ctx,
        Err(e) => {
            return (
                Err(TxProcessingError::CannotResolveContext {
                    reason: e.to_string(),
                    raw_tx_hash,
                }),
                pre_exec_working_set.into(),
            );
        }
    };

    // Check that the transaction isn't a duplicate
    if let Err(e) = runtime.transaction_authorizer().check_uniqueness(
        &auth_data,
        &ctx,
        &mut pre_exec_working_set,
    ) {
        return (
            Err(TxProcessingError::Nonce {
                reason: e.to_string(),
                raw_tx_hash,
            }),
            pre_exec_working_set.into(),
        );
    }

    if let Err(e) = pre_exec_working_set.charge_gas(&S::gas_forced_sequencer_registration_cost()) {
        return (
            Err(TxProcessingError::CannotReserveGas {
                reason: e.to_string(),
                raw_tx_hash,
            }),
            pre_exec_working_set.into(),
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
                return (
                    Err(TxProcessingError::CannotReserveGas {
                        reason: reason.to_string(),
                        raw_tx_hash,
                    }),
                    pre_exec_working_set.into(),
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
