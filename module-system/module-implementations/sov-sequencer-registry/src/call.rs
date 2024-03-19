use anyhow::bail;
use sov_bank::Amount;
#[cfg(feature = "native")]
use sov_modules_api::macros::CliWalletArg;
use sov_modules_api::{CallResponse, Context, StateAccessor, WorkingSet};

use crate::{AllowedSequencer, SequencerRegistry};

/// This enumeration represents the available call messages for interacting with
/// the `sov-sequencer-registry` module.
#[cfg_attr(feature = "native", derive(schemars::JsonSchema), derive(CliWalletArg))]
#[cfg_attr(
    feature = "arbitrary",
    derive(arbitrary::Arbitrary, proptest_derive::Arbitrary)
)]
#[derive(
    Debug,
    PartialEq,
    Clone,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum CallMessage {
    /// Add a new sequencer to the sequencer registry.
    Register {
        /// The raw Da address of the sequencer you're registering.
        da_address: Vec<u8>,
        /// The inital balance of the sequencer.
        amount: Amount,
    },
    /// Increases the balance of the sequencer, transferring the funds from the sequencer account
    /// to the rollup.
    Deposit {
        /// The raw Da address of the sequencer.
        da_address: Vec<u8>,
        /// The amount to increase.
        amount: Amount,
    },
    /// Remove a sequencer from the sequencer registry.
    Exit {
        /// The raw Da address of the sequencer you're removing.
        da_address: Vec<u8>,
    },
}

impl<S: sov_modules_api::Spec, Da: sov_modules_api::DaSpec> SequencerRegistry<S, Da> {
    pub(crate) fn register(
        &self,
        da_address: &Da::Address,
        amount: Amount,
        context: &Context<S>,
        working_set: &mut WorkingSet<S>,
    ) -> anyhow::Result<CallResponse> {
        let sequencer = context.sender();
        self.register_sequencer(da_address, sequencer, amount, working_set)?;
        Ok(CallResponse::default())
    }

    pub(crate) fn exit(
        &self,
        da_address: &Da::Address,
        context: &Context<S>,
        working_set: &mut WorkingSet<S>,
    ) -> anyhow::Result<CallResponse> {
        let locker = &self.address;

        let sequencer = context.sender();

        let belongs_to = self
            .allowed_sequencers
            .get_or_err(da_address, working_set)?
            .rollup_address;

        if sequencer != &belongs_to {
            bail!("Unauthorized exit attempt from sequencer `{}`", sequencer);
        }

        let mut coins = self.get_coins_to_lock(working_set);

        // we still remove the sequencer from the registry, even if there is no balance
        coins.amount = self
            .get_sender_balance(da_address, working_set)
            .unwrap_or(0);

        self.delete(da_address, working_set);

        if coins.amount > 0 {
            self.bank
                .transfer_from(locker, sequencer, coins, working_set)?;
        }

        Ok(CallResponse::default())
    }

    pub(crate) fn delete(&self, da_address: &Da::Address, working_set: &mut impl StateAccessor) {
        self.allowed_sequencers.delete(da_address, working_set);

        if let Some(preferred_sequencer) = self.preferred_sequencer.get(working_set) {
            if da_address == &preferred_sequencer {
                self.preferred_sequencer.delete(working_set);
            }
        }
    }

    /// Increases the balance of the provided sender, updating the state of the registry.
    ///
    /// # Errors
    ///
    /// Will error when:
    ///
    /// - The provided sender is not allowed.
    /// - The provided sender doesn't have enough funds to increase its balance.
    /// - The amount overflows.
    pub(crate) fn increase_sender_balance(
        &self,
        sender: &Da::Address,
        amount: Amount,
        working_set: &mut WorkingSet<S>,
    ) -> anyhow::Result<CallResponse> {
        let AllowedSequencer {
            rollup_address,
            balance,
        } = match self.allowed_sequencers.get(sender, working_set) {
            Some(s) => s,
            None => bail!("The provided sender `{}` is not allowed", sender),
        };

        let locker = &self.address;

        let balance = match balance.checked_add(amount) {
            Some(b) => b,
            None => bail!(
                "The provided amount `{}` overflows with the given balance `{}`.",
                amount,
                balance
            ),
        };

        let mut coins = self.get_coins_to_lock(working_set);
        coins.amount = amount;

        self.bank
            .transfer_from(&rollup_address, locker, coins, working_set)?;

        self.allowed_sequencers.set(
            sender,
            &AllowedSequencer {
                rollup_address,
                balance,
            },
            working_set,
        );

        Ok(CallResponse::default())
    }
}
