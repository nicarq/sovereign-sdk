use sov_bank::{config_gas_token_id, Coins, IntoPayable, Payable};
use sov_modules_api::capabilities::AuthorizeSequencerError;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::{DaSpec, Spec, TxScratchpad};

use crate::{AllowedSequencer, AllowedSequencerError, SequencerRegistry};

impl<S: Spec> SequencerRegistry<S> {
    /// Checks whether `sender` is a registered sequencer with enough staked amount.
    pub fn authorize_sequencer(
        &self,
        sender: &<S::Da as DaSpec>::Address,
        min_bond: u64,
        scratchpad: &mut TxScratchpad<S::Storage>,
    ) -> Result<AllowedSequencer<S>, AuthorizeSequencerError> {
        let allowed_sequencer = match self.is_sender_allowed(sender, scratchpad) {
            Ok(seq) => seq,
            Err(e) => return Err(AuthorizeSequencerError { reason: e.into() }),
        };

        if allowed_sequencer.balance < min_bond {
            let err = AllowedSequencerError::InsufficientStakeAmount {
                bond_amount: allowed_sequencer.balance,
                minimum_bond_amount: min_bond,
            };
            return Err(AuthorizeSequencerError { reason: err.into() });
        }

        Ok(allowed_sequencer)
    }

    /// Transfers a portion of the sequencer's stake to the beneficiary and decreases the staked balance.
    pub fn remove_part_of_the_stake(
        &self,
        sequencer: &<S::Da as DaSpec>::Address,
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

    /// Increases the staked balance of the sequencer by transferring the given amount from the sender to the SequencerRegistry module.
    pub fn add_to_stake(
        &self,
        sender: impl Payable<S>,
        sequencer: &<S::Da as DaSpec>::Address,
        amount: u64,
        state: &mut TxScratchpad<S::Storage>,
    ) -> anyhow::Result<()> {
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
                .transfer_from(sender, self.id.to_payable(), coins, state)?;

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
