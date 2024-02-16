use core::str::FromStr;

use anyhow::bail;
use sov_modules_api::macros::config_constant;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{Context, Gas, GasMeter, ModuleInfo, StateCheckpoint};

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
pub struct BankTxHook<C: Context> {
    /// The tx sender address
    pub sender: C::Address,
    /// The sequencer address
    pub sequencer: C::Address,
}

impl<C: Context> Bank<C> {
    /// Reserve the gas for a transaction.
    pub fn reserve_gas(
        &self,
        tx: &Transaction<C>,
        gas_price: &<C::Gas as Gas>::Price,
        payer: &C::Address,
        state_checkpoint: &mut StateCheckpoint<C>,
    ) -> Result<GasMeter<C::Gas>, anyhow::Error> {
        // TODO(@vlopes11) - this calulation diverges from EIP 1559
        if tx
            .max_gas_price()
            .map(|max_gas_price| max_gas_price < gas_price)
            .unwrap_or(false)
        {
            bail!("The maximum gas price ({:?}) was insufficient to cover the current price ({:?}) for ", tx.max_gas_price(), gas_price)
        }
        let amount = tx.gas_limit().saturating_add(tx.gas_tip());
        if amount > 0 {
            let token_address = C::Address::from_str(GAS_TOKEN_ADDRESS)
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
        _tx: &Transaction<C>,
        gas_meter: &sov_modules_api::GasMeter<C::Gas>,
        payer: &C::Address,
        state_checkpoint: &mut StateCheckpoint<C>,
    ) {
        let amount = gas_meter.remaining_funds();

        if amount > 0 {
            let token_address = C::Address::from_str(GAS_TOKEN_ADDRESS)
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
