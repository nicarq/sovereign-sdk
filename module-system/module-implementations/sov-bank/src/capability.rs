use sov_modules_api::transaction::{AuthenticatedTransactionData, TransactionConsumption};
use sov_modules_api::{Gas, GasMeter, Spec, StateCheckpoint, TxGasMeter};
use thiserror::Error;

use crate::utils::IntoPayable;
use crate::{Bank, Coins, Payable, GAS_TOKEN_ID};

/// Error types that can be raised by the `reserve_gas` method
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ReserveGasError {
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

/// The [`Bank::reserve_gas`] and [`Bank::refund_remaining_gas`] are used to reserve and then lock transaction base gas and tip
impl<S: Spec> Bank<S> {
    /// Reserve the gas necessary to execute a transaction. The gas is locked at the bank's address
    /// This method loosely follow the-EIP 1559 gas price calculation.
    pub fn reserve_gas(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
        payer: &S::Address,
        pre_execution_checks_meter: &impl GasMeter<S::Gas>,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> Result<TxGasMeter<S::Gas>, ReserveGasError> {
        let balance = self
            .get_balance_of(&payer.clone(), GAS_TOKEN_ID, state_checkpoint)
            .ok_or(ReserveGasError::AccountDoesNotExist)?;

        // the signer must be able to afford the transaction
        if balance < tx.max_fee {
            return Err(ReserveGasError::InsufficientBalanceToReserveGas);
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
            state_checkpoint,
        )
        .expect("Since the balance is checked above, this should be infallible. This is a bug");

        // We compute the gas amount that the transaction should consume.
        let amount_to_consume = match &tx.gas_limit {
            // If the user has provided a gas limit, we use the `gas_limit * gas_price` as the amount to consume (EIP-1559).
            Some(gas_limit) => {
                // We need to check the gas price in case the user has provided a gas limit.
                if tx.max_fee < gas_limit.value(gas_price) {
                    return Err(ReserveGasError::CurrentGasPriceTooHigh);
                }

                gas_limit.value(gas_price)
            }
            // If the user has not provided a gas limit, we use the `max_fee` as the amount to consume.
            None => tx.max_fee,
        };

        let mut gas_meter = TxGasMeter::new(amount_to_consume, gas_price.clone());

        gas_meter
            .charge_gas(pre_execution_checks_meter.gas_used())
            .map_err(|err| {
                ReserveGasError::InsufficientGasForPreExecutionChecks(err.to_string())
            })?;

        Ok(gas_meter)
    }

    // Computes and allocates the transaction reward to the base fee and the tip recipients.
    /// The transaction reward is computed following the EIP-1559 specification.
    /// Returns the gas consumed in a [`TransactionConsumption`] struct.
    /// The [`TxGasMeter`] is consumed to ensure that the transaction reward is only computed once at the end of the transaction execution.
    pub fn consume_gas_and_allocate_rewards(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_meter: sov_modules_api::TxGasMeter<S::Gas>,
        // The address that receives the base fee. Typically, this is the module id of either the `ProverIncentives` or the `AttesterIncentives` module.
        base_fee_recipient: &impl Payable<S>,
        // The address that receives the transaction tip. Typically, the module id of the `SequencerRegistry` module.
        tip_recipient: &impl Payable<S>,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> TransactionConsumption {
        let transaction_reward = tx.transaction_reward(gas_meter);

        self.transfer_from(
            self.id.to_payable(),
            base_fee_recipient.as_token_holder(),
            Coins {
                amount: transaction_reward.base_fee(),
                token_id: GAS_TOKEN_ID,
            },
            state_checkpoint,
        )
        .expect("Transferring the consumed base fee gas is infallible");

        self.transfer_from(
            self.id.to_payable(),
            tip_recipient.as_token_holder(),
            Coins {
                amount: transaction_reward.priority_fee(),
                token_id: GAS_TOKEN_ID,
            },
            state_checkpoint,
        )
        .expect("Transferring the consumed gas tip is infallible");

        transaction_reward
    }

    /// Refunds any remaining gas to the payer from the bank module after the transaction is processed.
    pub fn refund_remaining_gas(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        payer: &S::Address,
        consumption: &TransactionConsumption,
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
