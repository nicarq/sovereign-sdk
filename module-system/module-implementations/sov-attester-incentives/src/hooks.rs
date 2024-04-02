use sov_modules_api::transaction::Transaction;
use sov_modules_api::{DaSpec, Gas, GasMeter, Spec, StateCheckpoint};

use crate::AttesterIncentives;

/// The [`AttesterIncentives::reserve_gas`] and [`AttesterIncentives::refund_remaining_gas`] are used to lock transaction base gas
/// to the incentives module so that the attesters can be rewarded for their work.
/// These methods are the optimistic execution equivalent of the ones in `ProverIncentives`.
impl<S: Spec, Da: DaSpec> AttesterIncentives<S, Da> {
    /// Reserve the gas for a transaction.
    pub fn reserve_gas(
        &self,
        tx: &Transaction<S>,
        gas_price: &<S::Gas as Gas>::Price,
        payer: &S::Address,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) -> Result<GasMeter<S::Gas>, anyhow::Error> {
        self.bank
            .reserve_gas_to_address(tx, gas_price, payer, &self.address, state_checkpoint)
    }

    /// Refunds any remaining gas to the payer after the transaction is processed.
    pub fn refund_remaining_gas(
        &self,
        tx: &Transaction<S>,
        gas_meter: &sov_modules_api::GasMeter<S::Gas>,
        payer: &S::Address,
        state_checkpoint: &mut StateCheckpoint<S>,
    ) {
        self.bank.refund_remaining_gas_from_address(
            tx,
            gas_meter,
            payer,
            &self.address,
            state_checkpoint,
        );
    }
}
