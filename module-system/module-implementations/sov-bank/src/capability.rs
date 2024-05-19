use sov_modules_api::transaction::AuthenticatedTransactionData;
use sov_modules_api::{Gas, GasMeter, Spec, StateCheckpoint, TransactionConsumption, WorkingSet};
use thiserror::Error;

use crate::utils::IntoPayable;
use crate::{Bank, Coins, Payable, GAS_TOKEN_ID};

/// Error types that can be raised by the `reserve_gas` method
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ReserveGasErrorReason {
    #[error("The payer does not have an account in the `Bank` module for the gas token")]
    /// The payer does not have an account in the `Bank` module for the gas token
    AccountDoesNotExist,
    #[error("Insufficient balance to pay for the transaction gas")]
    /// The sender balance is not high enough to pay for the gas.
    InsufficientBalanceToReserveGas,
    #[error("The current gas price is too high to cover the maximum fee for the transaction")]
    /// The current gas price is too high to cover the maximum fee for the transaction.
    CurrentGasPriceTooHigh,
    #[error("Insufficient gas locked to pay for pre execution checks. Error: {0}")]
    /// Insufficient gas locked in the transaction to cover pre-execution checks such as signature checks or transaction
    /// deserialization
    InsufficientGasForPreExecutionChecks(String),
}

/// Error type that can be raised by the `reserve_gas` method
pub struct ReserveGasError<S: Spec> {
    /// The reason for the error.
    pub reason: ReserveGasErrorReason,
    /// The state checkpoint at the time of the error.
    pub state_checkpoint: StateCheckpoint<S>,
}

/// The [`Bank::reserve_gas`] and [`Bank::refund_remaining_gas`] are used to reserve and then lock transaction base gas and tip
impl<S: Spec> Bank<S> {
    /// Reserve the gas necessary to execute a transaction. The gas is locked at the bank's address
    /// This method loosely follow the-EIP 1559 gas price calculation.
    #[allow(clippy::result_large_err)]
    pub fn reserve_gas(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
        payer: &S::Address,
        pre_execution_checks_meter: &impl GasMeter<S::Gas>,
        mut state_checkpoint: StateCheckpoint<S>,
    ) -> Result<WorkingSet<S>, ReserveGasError<S>> {
        // We need to do the explicit check (outside of a closure) because otherwise `state_checkpoint` would be captured.
        let balance = match self.get_balance_of(&payer.clone(), GAS_TOKEN_ID, &mut state_checkpoint)
        {
            Some(balance) => balance,
            None => {
                return Err(ReserveGasError::<S> {
                    state_checkpoint,
                    reason: ReserveGasErrorReason::AccountDoesNotExist,
                })
            }
        };

        // the signer must be able to afford the transaction
        if balance < tx.max_fee {
            return Err(ReserveGasError::<S> {
                state_checkpoint,
                reason: ReserveGasErrorReason::InsufficientBalanceToReserveGas,
            });
        }

        if tx.max_fee == 0 {
            tracing::warn!(
                signer_default_address = ?tx.default_address,
                nonce = tx.nonce,
                %payer,
                "Trying to reserve gas for tx with zero max fee"
            );
        }

        // We lock the `max_fee` amount into the `Bank` module.
        // We actually **need** to do that transfer because the payer account balance may change during the execution of the transaction.
        self.transfer_from(
            payer,
            self.id.to_payable(),
            Coins {
                amount: tx.max_fee,
                token_id: GAS_TOKEN_ID,
            },
            &mut state_checkpoint,
        )
        .expect("Since the balance is checked above, this should be infallible. This is a bug");

        if let Some(gas_limit) = &tx.gas_limit {
            // We need to check the gas price in case the user has provided a gas limit.
            if tx.max_fee < gas_limit.value(gas_price) {
                return Err(ReserveGasError::<S> {
                    state_checkpoint,
                    reason: ReserveGasErrorReason::CurrentGasPriceTooHigh,
                });
            }
        }

        let mut ws = state_checkpoint.to_revertable(tx, gas_price);

        match ws.charge_gas(pre_execution_checks_meter.gas_used()) {
            Ok(_) => Ok(ws),
            Err(err) => {
                let (checkpoint, _) = ws.revert();
                Err(ReserveGasError::<S> {
                    state_checkpoint: checkpoint,
                    reason: ReserveGasErrorReason::InsufficientGasForPreExecutionChecks(
                        err.to_string(),
                    ),
                })
            }
        }
    }

    /// Computes and allocates the gas consumed by the transaction to the base fee and the tip recipients.
    pub fn allocate_consumed_gas(
        &self,
        // The address that receives the base fee. Typically, this is the module id of either the `ProverIncentives` or the `AttesterIncentives` module.
        base_fee_recipient: &impl Payable<S>,
        // The address that receives the transaction tip. Typically, the module id of the `SequencerRegistry` module.
        tip_recipient: &impl Payable<S>,
        consumed_gas: &TransactionConsumption<S::Gas>,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        self.transfer_from(
            self.id.to_payable(),
            base_fee_recipient.as_token_holder(),
            Coins {
                amount: consumed_gas.base_fee_value(),
                token_id: GAS_TOKEN_ID,
            },
            state_checkpoint,
        )
        .expect("Transferring the consumed base fee gas is infallible");

        self.transfer_from(
            self.id.to_payable(),
            tip_recipient.as_token_holder(),
            Coins {
                amount: consumed_gas.priority_fee(),
                token_id: GAS_TOKEN_ID,
            },
            state_checkpoint,
        )
        .expect("Transferring the consumed gas tip is infallible");
    }

    /// Refunds any remaining gas to the payer from the bank module after the transaction is processed.
    pub fn refund_remaining_gas(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        payer: &S::Address,
        consumption: &TransactionConsumption<S::Gas>,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        // We refund the payer. We need to give back the remaining funds on the gas meter, plus the unspent tip.
        // This is also the maximum fee minus everything that was spent for the tip and base fee (ie the total reward).
        let total_consumption = consumption.total_consumption();
        let max_fee = tx.max_fee;

        let amount = max_fee
            .checked_sub(total_consumption)
            .unwrap_or_else(|| panic!("The total consumption {total_consumption} should always be less than the max fee {max_fee}"));

        self.transfer_from(
            self.id.to_payable(),
            payer,
            Coins {
                amount,
                token_id: GAS_TOKEN_ID,
            },
            state_checkpoint,
        )
        .expect("Refunding unspent gas is infallible");
    }
}
