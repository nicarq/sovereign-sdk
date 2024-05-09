use std::cmp::min;

use sov_modules_api::transaction::AuthenticatedTransactionData;
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
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> Result<TxGasMeter<S::Gas>, ReserveGasError> {
        let balance = self
            .get_balance_of(&payer.clone(), GAS_TOKEN_ID, state_checkpoint)
            .ok_or(ReserveGasError::AccountDoesNotExist)?;

        // the signer must be able to afford the transaction
        if balance < tx.max_fee() {
            return Err(ReserveGasError::InsufficientBalanceToReserveGas);
        }

        if tx.max_fee() == 0 {
            tracing::warn!(
                signer_default_address = %tx.default_address(),
                nonce = tx.nonce(),
                %payer,
                "Trying to reserve gas for tx with zero max fee"
            );
        }

        // We lock the `max_fee` amount into the `Bank` module.
        self.transfer_from(
            payer,
            self.id.to_payable(),
            Coins {
                amount: tx.max_fee(),
                token_id: GAS_TOKEN_ID,
            },
            state_checkpoint,
        )
        .expect("Since the balance is checked above, this should be infallible. This is a bug");

        // We compute the gas amount that the transaction should consume.
        let amount_to_consume = match tx.gas_limit() {
            // If the user has provided a gas limit, we use the `gas_limit * gas_price` as the amount to consume (EIP-1559).
            Some(gas_limit) => {
                // We need to check the gas price in case the user has provided a gas limit.
                if tx.max_fee() < gas_limit.value(gas_price) {
                    return Err(ReserveGasError::CurrentGasPriceTooHigh);
                }

                gas_limit.value(gas_price)
            }
            // If the user has not provided a gas limit, we use the `max_fee` as the amount to consume.
            None => tx.max_fee(),
        };

        let gas_meter = TxGasMeter::new(amount_to_consume, gas_price.clone());

        Ok(gas_meter)
    }

    /// Refunds any remaining gas to the payer from the bank module after the transaction is processed.
    pub fn refund_remaining_gas(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_meter: &sov_modules_api::TxGasMeter<S::Gas>,
        payer: &S::Address,
        // The address that receives the base fee. Typically, this is the module id of either the `ProverIncentives` or the `AttesterIncentives` module.
        base_fee_recipient: &impl Payable<S>,
        // The address that receives the transaction tip. Typically, the module id of the `SequencerRegistry` module.
        tip_recipient: &impl Payable<S>,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        // We transfer the consumed base fee to the base fee recipient address.
        let base_fee = gas_meter.gas_used().value(gas_meter.gas_price());

        self.transfer_from(
            self.id.to_payable(),
            base_fee_recipient.as_token_holder(),
            Coins {
                amount: base_fee,
                token_id: GAS_TOKEN_ID,
            },
            state_checkpoint,
        )
        .expect("Transferring the consumed base fee gas is infallible");

        // We compute the `max_priority_fee_bips` by applying the `priority_fee_per_gas` to the consumed gas.
        let max_priority_fee_bips = tx
            .max_priority_fee_bips()
            .apply(base_fee)
            // if the computation overflows, we return the max fee - we always have `priority_fee <= tx.max_priority_fee_bips() <= tx.max_fee()`
            .unwrap_or(tx.max_fee());

        // The tip is the minimum of the remaining gas allocated to the transaction and the maximum priority fee per gas.
        // We transfer the tip to the tip recipient address.
        let tip = min(max_priority_fee_bips, tx.max_fee() - base_fee);

        self.transfer_from(
            self.id.to_payable(),
            tip_recipient.as_token_holder(),
            Coins {
                amount: tip,
                token_id: GAS_TOKEN_ID,
            },
            state_checkpoint,
        )
        .expect("Transferring the consumed gas tip is infallible");

        // We refund the payer. We need to give back the remaining funds on the gas meter, plus the unspent tip.
        // This is also the maximum fee minus everything that was spent for the tip and base fee.
        let amount = tx.max_fee() - tip - base_fee;

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
