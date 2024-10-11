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
