use sov_bank::{config_gas_token_id, Coins, IntoPayable, Payable};
use sov_modules_api::capabilities::AuthorizeSequencerError;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{DaSpec, Gas, Spec, TxScratchpad};

use crate::{AllowedSequencer, SequencerRegistry};

impl<S: Spec, Da: DaSpec> SequencerRegistry<S, Da> {
    /// Checks whether `sender` is a registered sequencer with enough staked amount.
    pub fn authorize_sequencer(
        &self,
        sender: &Da::Address,
        base_fee_per_gas: &<S::Gas as Gas>::Price,
        scratchpad: &mut TxScratchpad<S::Storage>,
    ) -> Result<AllowedSequencer<S>, AuthorizeSequencerError> {
        let allowed_sequencer = match self.is_sender_allowed(sender, base_fee_per_gas, scratchpad) {
            Ok(seq) => seq,
            Err(e) => return Err(AuthorizeSequencerError { reason: e.into() }),
        };

        Ok(allowed_sequencer)
    }

    /// Penalizes the sequencer.
    pub fn penalize_sequencer(
        &self,
        sender: &Da::Address,
        reason: impl std::fmt::Display,
        remaining_stake: u64,
        state: &mut TxScratchpad<S::Storage>,
    ) {
        if let Some(AllowedSequencer {
            address,
            balance: _,
        }) = self
            .allowed_sequencers
            .get(sender, state)
            .unwrap_infallible()
        {
            tracing::info!(
                sequencer = %address,
                remaining_stake = %remaining_stake,
                reason = %reason,
                "The sequencer was penalized",
            );

            self.allowed_sequencers
                .set(
                    sender,
                    &AllowedSequencer {
                        address,
                        balance: remaining_stake,
                    },
                    state,
                )
                .unwrap_infallible();
        }
    }

    /// Transfers a portion of the sequencer's stake to the beneficiary and decreases the staked balance.
    pub fn remove_part_of_the_stake(
        &self,
        sequencer: &Da::Address,
        beneficiary: impl Payable<S>,
        amount: u64,
        state: &mut TxScratchpad<S::Storage>,
    ) -> Result<(), anyhow::Error> {
        if let Some(AllowedSequencer { address, balance }) = self
            .allowed_sequencers
            .get(sequencer, state)
            .unwrap_infallible()
        {
            let new_balance = balance.checked_sub(amount).ok_or_else(|| {
                anyhow::anyhow!(
                    "Sequencer {} stake is too low. Balance {}, amount: {}",
                    sequencer,
                    balance,
                    amount
                )
            })?;

            let coins = Coins {
                amount,
                token_id: config_gas_token_id(),
            };

            self.bank
                .transfer_from(self.id.to_payable(), beneficiary, coins, state)?;

            self.allowed_sequencers
                .set(
                    sequencer,
                    &AllowedSequencer {
                        address,
                        balance: new_balance,
                    },
                    state,
                )
                .unwrap_infallible();

            Ok(())
        } else {
            anyhow::bail!("Sequencer {} is not registered", sequencer)
        }
    }

    /// Increases the staked balance of the sequencer by transferring the given amount from the user to the SequencerRegistry module.
    pub fn add_to_stake(
        &self,
        user: &S::Address,
        sequencer: &Da::Address,
        amount: u64,
        state: &mut TxScratchpad<S::Storage>,
    ) -> Result<(), anyhow::Error> {
        if let Some(AllowedSequencer { address, balance }) = self
            .allowed_sequencers
            .get(sequencer, state)
            .unwrap_infallible()
        {
            let new_balance = balance.checked_add(amount).ok_or_else(|| {
                anyhow::anyhow!(
                    "Sequencer {}: overflow error unable to increase sequencer's stake",
                    sequencer
                )
            })?;

            let coins = Coins {
                amount,
                token_id: config_gas_token_id(),
            };

            self.bank
                .transfer_from(user, self.id.to_payable(), coins, state)?;

            self.allowed_sequencers
                .set(
                    sequencer,
                    &AllowedSequencer {
                        address,
                        balance: new_balance,
                    },
                    state,
                )
                .unwrap_infallible();

            Ok(())
        } else {
            anyhow::bail!("Sequencer {} is not registered", sequencer)
        }
    }
}

#[cfg(test)]
mod tests {
    use sov_modules_api::{BasicGasMeter, GasMeter, GasPrice, GasUnit};

    #[test]
    fn charge_gas_should_fail_if_not_enough_funds() {
        let gas_price = GasPrice::<2>::from([1; 2]);

        let mut gas_meter = BasicGasMeter::new(0, gas_price.clone());

        assert!(
            gas_meter.charge_gas(&GasUnit::<2>::from([100; 2])).is_err(),
            "The gas meter should not be able to charge gas if there is not enough funds"
        );
    }

    #[test]
    fn refund_gas_should_fail_if_not_enough_funds_consumed() {
        let gas_price = GasPrice::<2>::from([1; 2]);

        let mut gas_meter = BasicGasMeter::new(100, gas_price.clone());

        assert!(
            gas_meter.refund_gas(&GasUnit::<2>::from([100; 2])).is_err(),
            "The gas meter should not be able to refund gas if there is not enough gas consumed"
        );
    }

    #[test]
    fn try_charge_gas() {
        const REMAINING_FUNDS: u64 = 100;
        let gas_price = GasPrice::<2>::from([1; 2]);

        let mut gas_meter = BasicGasMeter::new(REMAINING_FUNDS, gas_price.clone());
        assert!(
            gas_meter
                .charge_gas(&GasUnit::<2>::from([REMAINING_FUNDS / 2; 2]))
                .is_ok(),
            "It should be possible to charge gas"
        );
        assert_eq!(
            gas_meter.gas_info().gas_used,
            GasUnit::from([REMAINING_FUNDS / 2; 2]),
            "The gas used should be the same as the gas charged"
        );
        assert_eq!(gas_meter.gas_info().gas_price, gas_price);
        assert_eq!(
            gas_meter.gas_info().remaining_funds,
            0,
            "There should be no more gas left in the meter"
        );

        assert!(
            gas_meter.charge_gas(&GasUnit::<2>::from([1; 2])).is_err(),
            "There should be no more gas left in the meter, hence charging more gas should fail"
        );
    }

    #[test]
    fn try_refund_gas() {
        const REMAINING_FUNDS: u64 = 100;
        let gas_price = GasPrice::from([1; 2]);

        let mut gas_meter = BasicGasMeter::new(REMAINING_FUNDS, gas_price);
        assert!(
            gas_meter
                .charge_gas(&GasUnit::<2>::from([REMAINING_FUNDS / 2; 2]))
                .is_ok(),
            "There should be enough gas left in the meter to charge"
        );
        assert_eq!(
            gas_meter.gas_info().remaining_funds,
            0,
            "There should be no more gas left in the meter"
        );

        assert!(
            gas_meter
                .refund_gas(&GasUnit::from([REMAINING_FUNDS / 4; 2]))
                .is_ok(),
            "Enough gas should have been consumed to be refunded",
        );

        assert_eq!(
            &gas_meter.gas_info().gas_used,
            &GasUnit::from([REMAINING_FUNDS / 4; 2],),
            "The gas used amount should have decreased"
        );

        assert_eq!(
            gas_meter.gas_info().remaining_funds,
            REMAINING_FUNDS / 2,
            "Half of the gas should be refunded"
        );
    }
}
