//! The `sov-sequencer-registry` module is responsible for sequencer
//! registration, slashing, and rewards. At the moment, only a centralized
//! sequencer is supported. The sequencer's address and bond are registered
//! during the rollup deployment.
//!
//! The module implements the [`sov_modules_api::hooks::ApplyBatchHooks`] trait.

#![deny(missing_docs)]
mod call;
mod event;
mod genesis;
mod hooks;

#[cfg(feature = "native")]
mod rpc;
use borsh::{BorshDeserialize, BorshSerialize};
pub use call::*;
pub use genesis::*;
#[cfg(feature = "native")]
pub use rpc::*;
use serde::{Deserialize, Serialize};
use sov_bank::{Amount, Coins, IntoPayable, GAS_TOKEN_ID};
use sov_modules_api::{
    CallResponse, Context, Error, EventEmitter, ModuleId, ModuleInfo, Spec, StateAccessor,
    StateCheckpoint, StateMap, StateValue, WorkingSet,
};
use sov_state::codec::BcsCodec;

use crate::event::Event;

/// An allowed sequencer for a rollup.
#[derive(Debug, Clone, Serialize, Deserialize, BorshSerialize, BorshDeserialize, Eq, PartialEq)]
#[serde(bound = "S::Address: serde::Serialize + serde::de::DeserializeOwned")]
pub(crate) struct AllowedSequencer<S: Spec> {
    /// The rollup address of the sequencer.
    pub address: S::Address,
    /// The staked balance of the sequencer.
    pub balance: Amount,
}

/// Reason why sequencer was slashed.
#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SlashingReason {
    /// This status indicates problem with batch deserialization.
    InvalidBatchEncoding,
    /// Stateless verification failed, for example deserialized transactions have invalid signatures.
    StatelessVerificationFailed,
    /// This status indicates problem with transaction deserialization.
    InvalidTransactionEncoding,
}

/// The `sov-sequencer-registry` module `struct`.
#[cfg_attr(feature = "native", derive(sov_modules_api::ModuleCallJsonSchema))]
#[derive(Clone, ModuleInfo)]
pub struct SequencerRegistry<S: Spec, Da: sov_modules_api::DaSpec> {
    /// The ID of the `sov_sequencer_registry` module.
    #[id]
    pub(crate) id: ModuleId,

    /// Reference to the Bank module.
    #[module]
    pub(crate) bank: sov_bank::Bank<S>,

    /// The minimum bond for a sequencer to send transactions.
    /// TODO(@theochap): This should be expressed in gas units.
    #[state]
    pub minimum_bond: StateValue<Amount>,

    /// Only batches from sequencers from this list are going to be processed.
    /// We need to map the DA address to the rollup address because the sequencer interacts with the rollup
    /// through the DA layer.
    #[state]
    pub(crate) allowed_sequencers: StateMap<Da::Address, AllowedSequencer<S>, BcsCodec>,

    /// Optional preferred sequencer.
    /// If set, batches from this sequencer will be processed first in block,
    /// So this sequencer can guarantee soft confirmation time for transactions
    #[state]
    pub(crate) preferred_sequencer: StateValue<Da::Address, BcsCodec>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
/// Result of applying a blob, from sequencer's point of view.
pub enum SequencerOutcome<Da: sov_modules_api::DaSpec> {
    /// The blob was applied successfully and the operation is concluded.
    Rewarded {
        /// The number of tokens to award
        amount: u64,
    },
    /// The sequencer incurred a net penalty as a result of invalid (but not provably malicious) trabsactions.
    Penalized {
        /// The number of tokens to confiscate
        amount: u64,
    },
    /// The blob was *not* applied successfully. The sequencer has been slashed
    /// as a result of the invalid blob.
    Slashed {
        /// The address of the sequencer that was slashed.
        sequencer: Da::Address,
    },
}

impl<S: Spec, Da: sov_modules_api::DaSpec> sov_modules_api::Module for SequencerRegistry<S, Da> {
    type Spec = S;

    type Config = SequencerConfig<S, Da>;

    type CallMessage = CallMessage;

    type Event = Event<S>;

    fn genesis(&self, config: &Self::Config, working_set: &mut WorkingSet<S>) -> Result<(), Error> {
        Ok(self.init_module(config, working_set)?)
    }

    fn call(
        &self,
        message: Self::CallMessage,
        context: &Context<Self::Spec>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<CallResponse, Error> {
        Ok(match message {
            CallMessage::Register { da_address, amount } => {
                let da_address = Da::Address::try_from(&da_address)?;
                self.register(&da_address, amount, context, working_set)
                    .map_err(|e| Error::ModuleError(e.into()))?
            }
            CallMessage::Deposit { da_address, amount } => {
                let da_address = Da::Address::try_from(&da_address)?;
                self.increase_sender_balance(&da_address, amount, working_set)
                    .map_err(|e| Error::ModuleError(e.into()))?
            }
            CallMessage::Exit { da_address } => {
                let da_address = Da::Address::try_from(&da_address)?;
                self.exit(&da_address, context, working_set)
                    .map_err(|e| Error::ModuleError(e.into()))?
            }
        })
    }
}

impl<S: Spec, Da: sov_modules_api::DaSpec> SequencerRegistry<S, Da> {
    /// Returns the minimum amount of tokens that the sequencer must lock.
    pub fn get_coins_to_lock(&self, working_set: &mut WorkingSet<S>) -> Coins {
        let amount = self
            .minimum_bond
            .get(working_set)
            .expect("The minimum bond should be set at genesis");
        Coins {
            amount,
            token_id: GAS_TOKEN_ID,
        }
    }

    /// Tries to register a sequencer by staking the provided amount of gas tokens.
    /// # Errors
    /// Will error
    ///
    /// - If the provided amount is below the minimum required to register a sequencer.
    /// - If the minimum bond is not set.
    /// - If the sender's account does not have enough funds to register itself as a sequencer.
    /// - If the sequencer is already registered.
    pub(crate) fn register_sequencer(
        &self,
        da_address: &Da::Address,
        address: &S::Address,
        amount: Amount,
        working_set: &mut WorkingSet<S>,
    ) -> Result<(), SequencerRegistryError<S, Da>> {
        if self
            .allowed_sequencers
            .get(da_address, working_set)
            .is_some()
        {
            return Err(SequencerRegistryError::SequencerAlreadyRegistered(
                address.clone(),
            ));
        }

        let minimum_bond = self
            .minimum_bond
            .get(working_set)
            .ok_or(SequencerRegistryError::NoMinimumBondSet)?;

        if amount < minimum_bond {
            return Err(SequencerRegistryError::InsufficientStakeAmount {
                bond_amount: amount,
                minimum_bond_amount: minimum_bond,
            });
        }

        let locker = &self.id;

        let coins = Coins {
            amount,
            token_id: GAS_TOKEN_ID,
        };

        self.bank
            .transfer_from(address, locker.to_payable(), coins, working_set)
            .map_err(|_| SequencerRegistryError::<S, Da>::InsufficientFundsToRegister(amount))?;

        self.allowed_sequencers.set(
            da_address,
            &AllowedSequencer {
                address: address.clone(),
                balance: amount,
            },
            working_set,
        );

        self.emit_event(
            working_set,
            "sequencer_registered",
            Event::<S>::Registered {
                sequencer: address.clone(),
                amount,
            },
        );

        Ok(())
    }

    /// Returns the preferred sequencer, or [`None`] it wasn't set.
    ///
    /// Read about [`SequencerConfig::is_preferred_sequencer`] to learn about
    /// preferred sequencers.
    pub fn get_preferred_sequencer(
        &self,
        working_set: &mut impl StateAccessor,
    ) -> Option<Da::Address> {
        self.preferred_sequencer.get(working_set)
    }

    /// Resolve a DA address to a rollup address.
    pub fn resolve_da_address(
        &self,
        address: &Da::Address,
        working_set: &mut impl StateAccessor,
    ) -> Option<S::Address> {
        self.allowed_sequencers
            .get(address, working_set)
            .map(|s| s.address)
    }

    /// Returns the rollup address of the preferred sequencer, or [`None`] it wasn't set.
    ///
    /// Read about [`SequencerConfig::is_preferred_sequencer`] to learn about
    /// preferred sequencers.
    pub fn get_preferred_sequencer_rollup_address(
        &self,
        working_set: &mut impl StateAccessor,
    ) -> Option<S::Address> {
        self.preferred_sequencer.get(working_set).map(|da_addr| {
            self.allowed_sequencers
                .get(&da_addr, working_set)
                .expect("Preferred Sequencer must have known address on rollup")
                .address
        })
    }

    /// Checks whether `sender` is a registered sequencer with enough staked amount.
    pub fn is_sender_allowed(
        &self,
        sender: &Da::Address,
        working_set: &mut impl StateAccessor,
    ) -> bool {
        let balance = match self.allowed_sequencers.get(sender, working_set) {
            Some(a) => a.balance,
            None => return false,
        };

        let min_bond = self
            .minimum_bond
            .get(working_set)
            .expect("The minimum bond should be set at genesis");

        if balance < min_bond {
            return false;
        }

        true
    }

    /// Returns the balance of the provided sender, if present.
    pub fn get_sender_balance(
        &self,
        sender: &Da::Address,
        working_set: &mut WorkingSet<S>,
    ) -> Option<Amount> {
        self.allowed_sequencers
            .get(sender, working_set)
            .map(|s| s.balance)
    }

    /// Slash the sequencer with the given address.
    pub fn slash_sequencer(&self, da_address: &Da::Address, working_set: &mut StateCheckpoint<S>) {
        self.delete(da_address, working_set);
    }
}
