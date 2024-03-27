use anyhow::bail;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{Gas, GasMeter, ModuleInfo, Spec, StateCheckpoint};

use crate::{Bank, Coins, GAS_TOKEN_ID};

/// The computed addresses of a pre-dispatch tx hook.
pub struct BankTxHook<S: Spec> {
    /// The tx sender address
    pub sender: S::Address,
    /// The sequencer address
    pub sequencer: S::Address,
}

impl<S: Spec> Bank<S> {
    /// Reserve the gas for a transaction.
    pub fn reserve_gas(
        &self,
        tx: &Transaction<S>,
        gas_price: &<S::Gas as Gas>::Price,
        payer: &S::Address,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> Result<GasMeter<S::Gas>, anyhow::Error> {
        // TODO(@vlopes11) - this calculation diverges from EIP 1559
        if tx
            .max_gas_price()
            .map(|max_gas_price| max_gas_price < gas_price)
            .unwrap_or(false)
        {
            bail!("The maximum gas price ({:?}) was insufficient to cover the current price ({:?}) for ", tx.max_gas_price(), gas_price)
        }
        // TODO(@theochap) This should be moved to sequencer registry
        let amount = tx.gas_limit().saturating_add(tx.gas_tip());
        if amount > 0 {
            let token_id = GAS_TOKEN_ID;
            let from = payer;
            let to = self.address();
            let coins = Coins { amount, token_id };
            // TODO(@preston-evans98) - in zk mode, this transfer should be earmarked for the prover
            self.transfer_from(from, to, coins, state_checkpoint)?;
        }
        let gas_meter = GasMeter::new(tx.gas_limit(), gas_price.clone());

        Ok(gas_meter)
    }

    /// Refunds any remaining gas to the payer after the transaction is processed.
    pub fn refund_remaining_gas(
        &self,
        _tx: &Transaction<S>,
        gas_meter: &sov_modules_api::GasMeter<S::Gas>,
        payer: &S::Address,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        let amount = gas_meter.remaining_funds();

        if amount > 0 {
            let token_id = GAS_TOKEN_ID;
            let from = self.address();
            let to = payer;
            let coins = Coins { amount, token_id };
            self.transfer_from(from, to, coins, state_checkpoint)
                .expect("Refunding unspent gas is infallible");
        }
    }
}
