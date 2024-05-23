use sov_bank::Amount;
use sov_modules_api::capabilities::AuthorizeSequencerError;
use sov_modules_api::{Gas, GasMeter, PreExecWorkingSet, Spec, TxScratchpad};
use thiserror::Error;

use crate::{AllowedSequencer, SequencerRegistry};

/// A struct that keeps track of the staked amount of a sequencer and the accumulated penalty amount.
/// The sequencer may get penalized for submitting invalid transactions, the penalties are accumulated
/// during execution in that struct. The remaining stake amount decreases as the penalties are accumulated.
///
/// The current amount staked by the sequencer is the sum of the
/// remaining stake amount and the accumulated penalty amount.
///
/// # Type-safety invariant
/// This struct should always ensure that the penalty amount is always below the total amount staked
/// for type safety purposes.
///
/// # Constructor
/// This struct can only be constructed by the [`SequencerRegistry::authorize_sequencer`] method.
pub struct SequencerStakeMeter<GU: Gas> {
    remaining_stake: Amount,
    penalty_accumulator: GU,
    gas_price: GU::Price,
}

/// Error raised when the sequencer is getting penalized an amount greater than its remaining stake.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("The remaining stake amount of the sequencer (value: {remaining_staked_amount}) is lower than the amount to penalize (gas price: {gas_price}, gas value: {amount_to_penalize})")]
pub struct SequencerStakeError<GU: Gas> {
    remaining_staked_amount: Amount,
    amount_to_penalize: GU,
    gas_price: GU::Price,
}

impl<GU: Gas> GasMeter<GU> for SequencerStakeMeter<GU> {
    fn charge_gas(&mut self, amount: &GU) -> Result<(), anyhow::Error> {
        let amount_value = amount.value(&self.gas_price);

        if amount_value > self.remaining_stake {
            let remaining_staked_amount = self.remaining_stake;
            let gas_price = self.gas_price.clone();
            let amount_to_charge = amount;
            anyhow::bail!(
                "The remaining stake amount of the sequencer (value: {remaining_staked_amount}) is lower than the amount to charge (gas price: {gas_price}, gas value: {amount_to_charge})",
            );
        }

        self.remaining_stake -= amount_value;
        self.penalty_accumulator.combine(amount);

        Ok(())
    }

    fn gas_used(&self) -> &GU {
        &self.penalty_accumulator
    }

    fn gas_price(&self) -> &GU::Price {
        &self.gas_price
    }

    fn remaining_funds(&self) -> u64 {
        self.remaining_stake
    }
}

impl<S: Spec, Da: sov_modules_api::DaSpec> SequencerRegistry<S, Da> {
    /// Checks whether `sender` is a registered sequencer with enough staked amount.
    /// If so, returns a [`SequencerStakeMeter`] which tracks the sequencer stake. Otherwise, returns a [`AuthorizeSequencerError`].
    pub fn authorize_sequencer(
        &self,
        sender: &Da::Address,
        base_fee_per_gas: &<S::Gas as Gas>::Price,
        mut scratchpad: TxScratchpad<S>,
    ) -> Result<PreExecWorkingSet<S, SequencerStakeMeter<S::Gas>>, AuthorizeSequencerError<S>> {
        let sequencer = match self.is_sender_allowed(sender, &mut scratchpad) {
            Ok(seq) => seq,
            Err(e) => {
                return Err(AuthorizeSequencerError {
                    tx_scratchpad: scratchpad,
                    reason: e.into(),
                })
            }
        };

        let seq_meter = SequencerStakeMeter::<S::Gas> {
            remaining_stake: sequencer.balance,
            penalty_accumulator: S::Gas::zero(),
            gas_price: base_fee_per_gas.clone(),
        };

        Ok(scratchpad.to_pre_exec_working_set(seq_meter))
    }

    /// Refunds some of the sequencer's staked amount.
    /// Only modifies the `remaining_stake` field of the [`SequencerStakeMeter`] to increase the remaining staked amount.
    /// The `gas_used` field of the [`SequencerStakeMeter`] is not modified.
    ///
    /// # Note
    /// Saturates if the sum of the refunded amount and remaining stake overflows.
    pub fn refund_sequencer(
        &self,
        sequencer_stake_meter: &mut SequencerStakeMeter<S::Gas>,
        refund_amount: u64,
    ) {
        sequencer_stake_meter.remaining_stake = sequencer_stake_meter
            .remaining_stake
            .saturating_add(refund_amount);
    }

    /// Penalizes the sequencer. In practice, sets its stake to the remaining stake tracked by the [`SequencerStakeMeter`].
    pub fn penalize_sequencer(
        &self,
        sender: &Da::Address,
        mut pre_exec_working_set: PreExecWorkingSet<S, SequencerStakeMeter<S::Gas>>,
    ) -> TxScratchpad<S> {
        if let Some(AllowedSequencer {
            address,
            balance: _,
        }) = self
            .allowed_sequencers
            .get(sender, &mut pre_exec_working_set)
        {
            let penalty_amount = pre_exec_working_set.gas_used_value();
            let remaining_stake = pre_exec_working_set.remaining_funds();

            tracing::info!(
                sequencer = %address,
                penalty_amount = ?penalty_amount,
                remaining_stake = %remaining_stake,
                "The sequencer was penalized"
            );

            self.allowed_sequencers.set(
                sender,
                &AllowedSequencer {
                    address,
                    balance: remaining_stake,
                },
                &mut pre_exec_working_set,
            );
        }

        pre_exec_working_set.into()
    }
}

#[cfg(test)]
mod tests {
    use sov_modules_api::{GasArray, GasMeter, GasPrice, GasUnit};

    use crate::SequencerStakeMeter;

    #[test]
    fn test_sequencer_stake_meter_enough_gas() {
        const INIT_STAKE: u64 = 100;
        let mut stake_meter = SequencerStakeMeter {
            remaining_stake: INIT_STAKE,
            penalty_accumulator: GasUnit::<2>::ZEROED,
            gas_price: GasPrice::<2>::from_slice(&[1; 2]),
        };

        stake_meter
            .charge_gas(&GasUnit::<2>::from_slice(&[INIT_STAKE / 2; 2]))
            .unwrap();
        assert_eq!(stake_meter.remaining_stake, 0);
    }

    #[test]
    fn test_sequencer_stake_meter_not_enough_gas() {
        const INIT_STAKE: u64 = 100;
        let mut stake_meter = SequencerStakeMeter {
            remaining_stake: INIT_STAKE,
            penalty_accumulator: GasUnit::<2>::ZEROED,
            gas_price: GasPrice::<2>::from_slice(&[1; 2]),
        };

        stake_meter
            .charge_gas(&GasUnit::<2>::from_slice(&[INIT_STAKE; 2]))
            .unwrap_err();
    }
}
