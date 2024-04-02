use anyhow::bail;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{Gas, GasMeter, Spec, StateCheckpoint};

use crate::{Bank, Coins, GAS_TOKEN_ID};

impl<S: Spec> Bank<S> {
    /// Reserve the gas for a transaction and lock it at the address `locking_address`.
    pub fn reserve_gas_to_address(
        &self,
        tx: &Transaction<S>,
        gas_price: &<S::Gas as Gas>::Price,
        payer: &S::Address,
        locking_address: &S::Address,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> Result<GasMeter<S::Gas>, anyhow::Error> {
        // TODO(@vlopes11) - this calculation diverges from EIP 1559
        if tx
            .max_gas_price()
            .map(|max_gas_price| max_gas_price < gas_price)
            .unwrap_or(false)
        {
            bail!(
            "The maximum gas price ({:?}) was insufficient to cover the current price ({:?}) for ",
            tx.max_gas_price(),
            gas_price
        )
        }
        // TODO(@theochap): this amount should be decomposed into base gas (goes to the prover incentives module) and
        // the tip (goes to the sequencer registry).
        let amount = tx.gas_limit().saturating_add(tx.gas_tip());
        if amount > 0 {
            let token_id = GAS_TOKEN_ID;
            let from = payer;
            let to = locking_address;
            let coins = Coins { amount, token_id };
            self.transfer_from(from, to, coins, state_checkpoint)?;
        }
        let gas_meter = GasMeter::new(tx.gas_limit(), gas_price.clone());

        Ok(gas_meter)
    }

    /// Refunds any remaining gas to the payer from the address `locking_address` after the transaction is processed.
    pub fn refund_remaining_gas_from_address(
        &self,
        _tx: &Transaction<S>,
        gas_meter: &sov_modules_api::GasMeter<S::Gas>,
        payer: &S::Address,
        locking_address: &S::Address,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        let amount = gas_meter.remaining_funds();

        if amount > 0 {
            let token_id = GAS_TOKEN_ID;
            let from = locking_address;
            let to = payer;
            let coins = Coins { amount, token_id };
            self.transfer_from(from, to, coins, state_checkpoint)
                .expect("Refunding unspent gas is infallible");
        }
    }
}
