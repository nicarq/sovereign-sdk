use sov_modules_api::capabilities::TryReserveGasError;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::transaction::{AuthenticatedTransactionData, ProverRewards, RemainingFunds};
use sov_modules_api::{Gas, Spec, StateAccessorError, TxScratchpad};
use thiserror::Error;

use crate::utils::IntoPayable;
use crate::{config_gas_token_id, Bank, Coins, Payable};

/// Error types that can be raised by the `reserve_gas` method
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ReserveGasError<S: Spec> {
    #[error("The payer {account} does not have an account in the `Bank` module for the gas token")]
    /// The payer does not have an account in the `Bank` module for the gas token
    AccountDoesNotExist {
        /// String representation of rollup address
        account: String,
    },
    #[error("Insufficient balance to pay for the transaction gas")]
    /// The sender balance is not high enough to pay for the gas.
    InsufficientBalanceToReserveGas,
    #[error("The current gas price is too high to cover the maximum fee for the transaction")]
    /// The current gas price is too high to cover the maximum fee for the transaction.
    CurrentGasPriceTooHigh,
    /// Impossible to transfer the gas from the payer to the bank
    #[error("Impossible to transfer the gas from the payer to the bank")]
    ImpossibleToTransferGas(String),
    /// Error occurred while accessing the state
    #[error("An error occurred while accessing the state: {0}")]
    StateAccessError(StateAccessorError<S::Gas>),
}

impl<S: Spec> From<ReserveGasError<S>> for TryReserveGasError {
    fn from(err: ReserveGasError<S>) -> Self {
        Self {
            reason: err.to_string(),
        }
    }
}

/// The [`Bank::reserve_gas`] and [`Bank::refund_remaining_gas`] are used to reserve and then lock transaction base gas and tip
impl<S: Spec> Bank<S> {
    /// Reserve the gas necessary to execute a transaction. The gas is locked at the bank's address
    /// This method loosely follows the-EIP 1559 gas price calculation.
    #[allow(clippy::result_large_err)]
    pub fn reserve_gas(
        &self,
        tx: &AuthenticatedTransactionData<S>,
        gas_price: &<S::Gas as Gas>::Price,
        payer: &S::Address,
        scratchpad: &mut TxScratchpad<S::Storage>,
    ) -> Result<(), ReserveGasError<S>> {
        // We need to do the explicit check (outside of a closure) because otherwise `state_checkpoint` would be captured.
        let balance = match self
            .get_balance_of(&payer.clone(), config_gas_token_id(), scratchpad)
            .unwrap_infallible()
        {
            Some(balance) => balance,
            None => {
                return Err(ReserveGasError::AccountDoesNotExist {
                    account: payer.to_string(),
                })
            }
        };

        // the signer must be able to afford the transaction
        if balance < tx.max_fee {
            return Err(ReserveGasError::InsufficientBalanceToReserveGas);
        }

        if tx.max_fee == 0 {
            tracing::warn!(
                %payer,
                "Trying to reserve gas for tx with zero max fee"
            );
        }

        // We lock the `max_fee` amount into the `Bank` module.
        // We actually **need** to do that transfer because the payer account balance may change during the execution of the transaction.
        if let Err(err) = self.transfer_from(
            payer,
            self.id.to_payable(),
            Coins {
                amount: tx.max_fee,
                token_id: config_gas_token_id(),
            },
            scratchpad,
        ) {
            return Err(ReserveGasError::ImpossibleToTransferGas(err.to_string()));
        }

        if let Some(gas_limit) = &tx.gas_limit {
            // We need to check the gas price in case the user has provided a gas limit.
            if tx.max_fee < gas_limit.value(gas_price) {
                return Err(ReserveGasError::CurrentGasPriceTooHigh);
            }
        }

        Ok(())
    }

    /// Computes and allocates the gas consumed by the transaction to the base fee and the tip recipients.
    pub fn reward_prover(
        &self,
        // The address that receives the base fee. Typically, this is the module id of either the `ProverIncentives` or the `AttesterIncentives` module.
        base_fee_recipient: &impl Payable<S>,

        base_fee: &ProverRewards,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
    ) {
        self.transfer_from(
            self.id.to_payable(),
            base_fee_recipient.as_token_holder(),
            Coins {
                amount: base_fee.0,
                token_id: config_gas_token_id(),
            },
            tx_scratchpad,
        )
        .expect("Transferring the consumed base fee gas is infallible");
    }

    /// Refunds any remaining gas to the payer from the bank module after the transaction is processed.
    pub fn refund_remaining_gas(
        &self,
        payer: &S::Address,
        remaining_funds: &RemainingFunds,
        tx_scratchpad: &mut TxScratchpad<S::Storage>,
    ) {
        // We refund the payer. We need to give back the remaining funds on the gas meter, plus the unspent tip.
        // This is also the maximum fee minus everything that was spent for the tip and base fee (ie the total reward).
        self.transfer_from(
            self.id.to_payable(),
            payer,
            Coins {
                amount: remaining_funds.0,
                token_id: config_gas_token_id(),
            },
            tx_scratchpad,
        )
        .expect("Refunding unspent gas is infallible");
    }
}
