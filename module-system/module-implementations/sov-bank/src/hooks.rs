use core::str::FromStr;

use anyhow::bail;
use sov_modules_api::macros::config_constant;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{Gas, GasMeter, ModuleInfo, Spec, StateCheckpoint};

use crate::{Bank, Coins};

#[config_constant]
// This constant is a fixed value, expected to be generated as
//
// ```rust
// let token_name = "sov-gas-token";
// let deployer = DEPLOYER_ADDRESS;
// let salt = 0;
// let computed = super::get_token_address::<DefaultContext>(token_name, &deployer, salt);
// ```
//
// TODO: fetch address as constant
// https://github.com/Sovereign-Labs/sovereign-sdk/issues/1234
const GAS_TOKEN_ADDRESS: &'static str;

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
        let amount = tx.gas_limit().saturating_add(tx.gas_tip());
        if amount > 0 {
            let token_address = S::Address::from_str(GAS_TOKEN_ADDRESS)
                .map_err(|_| anyhow::anyhow!("failed to parse gas token address"))?;
            let from = payer;
            let to = self.address();
            let coins = Coins {
                amount,
                token_address,
            };
            // TODO(@preston-evans98) - in zk mode, this transfer should be earmarked for the prover
            self.transfer_from(from, to, coins, state_checkpoint)?;
        }
        // TODO(@vlopes11) - fix confusion between available tokens and gas limit
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
            let token_address = S::Address::from_str(GAS_TOKEN_ADDRESS)
                .map_err(|_| "The rollup is misconfigured: the gas token address is invalid")
                .expect("failed to parse gas token address");
            let from = self.address();
            let to = payer;
            let coins = Coins {
                amount,
                token_address,
            };
            self.transfer_from(from, to, coins, state_checkpoint)
                .expect("Refunding unspent gas is infallible");
        }
    }
}
