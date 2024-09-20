//! The `sov-sequencer-registry` module is responsible for sequencer
//! registration, slashing, and rewards. At the moment, only a centralized
//! sequencer is supported. The sequencer's address and bond are registered
//! during the rollup deployment.
//!
//! The module implements the [`sov_modules_api::hooks::ApplyBatchHooks`] trait.
#![deny(missing_docs)]
mod call;
mod capabilities;
mod event;
mod registration;
pub use event::Event;

mod genesis;

use sov_bank::IntoPayable;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::registration_lib::{RegistrationError, StakeRegistration};
#[cfg(feature = "native")]
mod query;
use borsh::{BorshDeserialize, BorshSerialize};
pub use call::*;
pub use capabilities::SequencerStakeMeter;
pub use genesis::*;
#[cfg(feature = "native")]
pub use query::*;
use serde::{Deserialize, Serialize};
use sov_bank::{Amount, Coins, GAS_TOKEN_ID};
use sov_modules_api::capabilities::AllowedSequencer;
use sov_modules_api::{
    BasicAddress, CallResponse, Context, DaSpec, Error, Gas, GenesisState, InfallibleStateAccessor,
    Module, ModuleId, ModuleInfo, ModuleRestApi, Spec, StateAccessor, StateMap, StateReader,
    StateValue, TxScratchpad, TxState,
};
use sov_state::codec::BcsCodec;
use sov_state::User;
use thiserror::Error;

/// Errors that can be raised by the [`SequencerRegistry`] module during hooks execution.
#[derive(
    Debug, Clone, Error, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AllowedSequencerError {
    /// The amount of gas tokens that the sender is has staken is too low.
    #[error("The amount staked by the sequencer is less than the minimum bond. Amount currently staked: {bond_amount}, minimum bond amount: {minimum_bond_amount}")]
    InsufficientStakeAmount {
        /// The amount of gas tokens that the sender is has staken.
        bond_amount: Amount,
        /// The minimum amount of gas tokens that the sequencer must stake.
        minimum_bond_amount: Amount,
    },
    /// The sequencer is not registered.
    #[error("The sequencer is not registered")]
    NotRegistered,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
/// Reason why sequencer was slashed.
pub enum SlashingReason {
    /// This status indicates problem with batch deserialization.
    InvalidBatchEncoding,
    /// Stateless verification failed, for example deserialized transactions have invalid signatures.
    StatelessVerificationFailed,
    /// This status indicates problem with transaction deserialization.
    InvalidTransactionEncoding,
}

/// The `sov-sequencer-registry` module `struct`.
#[derive(Clone, ModuleInfo, ModuleRestApi)]
pub struct SequencerRegistry<S: Spec, Da: DaSpec> {
    /// The ID of the `sov_sequencer_registry` module.
    #[id]
    pub(crate) id: ModuleId,

    /// Reference to the Bank module.
    #[module]
    pub(crate) bank: sov_bank::Bank<S>,

    /// The minimum bond for a sequencer to send transactions.
    ///     
    /// This bond is expressed in gas units. When sequencers are submitting batches, they should
    /// have bonded at least the token value of this `minimum_bond` at the current `base_fee_per_gas`.
    #[state]
    pub minimum_bond: StateValue<S::Gas>,

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

/// A special error type that can be raised when calling a method from the sequencer registry
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CustomError<RollupAddress: BasicAddress, DaAddress: BasicAddress> {
    /// The sequencer tried to unregister itself during the execution of its own batch.
    #[error("Sequencers may not unregister during execution of their own batch")]
    CannotUnregisterDuringOwnBatch(DaAddress),

    #[error("The address provided as a parameter to the `exit` method does not match the transaction sender")]
    /// The address provided as a parameter to the `exit` method does not match the transaction sender.
    SuppliedAddressDoesNotMatchTxSender {
        /// The address provided as a parameter to the `exit` method.
        parameter: RollupAddress,
        /// The address of the transaction sender.
        sender: RollupAddress,
    },
}

/// The different errors that can be raised by the sequencer registry
#[allow(type_alias_bounds)]
pub type SequencerRegistryError<S: Spec, Da: DaSpec, ST: StateAccessor> = RegistrationError<
    S::Address,
    Da::Address,
    <ST as StateReader<User>>::Error,
    CustomError<S::Address, Da::Address>,
>;

impl<S: Spec, Da: DaSpec> Module for SequencerRegistry<S, Da> {
    type Spec = S;

    type Config = SequencerConfig<S, Da>;

    type CallMessage = CallMessage;

    type Event = Event<S>;

    fn genesis(
        &self,
        config: &Self::Config,
        state: &mut impl GenesisState<S>,
    ) -> Result<(), Error> {
        Ok(self.init_module(config, state)?)
    }

    fn call(
        &self,
        message: Self::CallMessage,
        context: &Context<Self::Spec>,
        state: &mut impl TxState<S>,
    ) -> Result<CallResponse, Error> {
        Ok(match message {
            CallMessage::Register { da_address, amount } => {
                let da_address = Da::Address::try_from(&da_address)?;
                self.register(&da_address, amount, context, state)
                    .map_err(|e| Error::ModuleError(e.into()))?
            }
            CallMessage::Deposit { da_address, amount } => {
                let da_address = Da::Address::try_from(&da_address)?;
                self.deposit(&da_address, amount, context, state)
                    .map_err(|e| Error::ModuleError(e.into()))?
            }
            CallMessage::Exit { da_address } => {
                let da_address = Da::Address::try_from(&da_address)?;
                self.exit(&da_address, context, state)
                    .map_err(|e| Error::ModuleError(e.into()))?
            }
        })
    }
}

impl<S: Spec, Da: DaSpec> SequencerRegistry<S, Da> {
    /// Returns the minimum amount of tokens that the sequencer must lock.
    pub fn get_coins_to_lock<Reader: TxState<S>>(
        &self,
        state: &mut Reader,
    ) -> Result<Coins, <Reader as StateReader<User>>::Error> {
        let amount = self
            .minimum_bond
            .get(state)?
            .expect("The minimum bond should be set at genesis");

        let amount_value = amount.value(state.gas_price());

        Ok(Coins {
            amount: amount_value,
            token_id: GAS_TOKEN_ID,
        })
    }

    /// Returns the preferred sequencer, or [`None`] it wasn't set.
    ///
    /// Read about [`SequencerConfig::is_preferred_sequencer`] to learn about
    /// preferred sequencers.
    pub fn get_preferred_sequencer<Reader: StateReader<User>>(
        &self,
        state: &mut Reader,
    ) -> Result<Option<Da::Address>, Reader::Error> {
        self.preferred_sequencer.get(state)
    }

    /// Resolve a DA address to a rollup address.
    pub fn resolve_da_address<Reader: StateReader<User>>(
        &self,
        address: &Da::Address,
        state: &mut Reader,
    ) -> Result<Option<S::Address>, Reader::Error> {
        self.allowed_sequencers
            .get(address, state)
            .map(|s| s.map(|s| s.address))
    }

    /// Returns the rollup address of the preferred sequencer, or [`None`] it wasn't set.
    ///
    /// Read about [`SequencerConfig::is_preferred_sequencer`] to learn about
    /// preferred sequencers.
    pub fn get_preferred_sequencer_rollup_address<Reader: StateReader<User>>(
        &self,
        state: &mut Reader,
    ) -> Result<Option<S::Address>, Reader::Error> {
        Ok(match self.preferred_sequencer.get(state)? {
            Some(da_addr) => Some(
                self.allowed_sequencers
                    .get(&da_addr, state)?
                    .expect("Preferred Sequencer must have known address on rollup")
                    .address,
            ),
            None => None,
        })
    }

    /// Checks whether `sender` is a registered sequencer with enough staked amount.
    /// If so, returns the allowed sequencer in a [`AllowedSequencer`] object.
    /// Otherwise, returns a [`AllowedSequencerError`].
    pub fn is_sender_allowed(
        &self,
        sender: &Da::Address,
        gas_price: &<S::Gas as Gas>::Price,
        state: &mut impl InfallibleStateAccessor,
    ) -> Result<AllowedSequencer<S>, AllowedSequencerError> {
        if let Some(sequencer) = self
            .allowed_sequencers
            .get(sender, state)
            .unwrap_infallible()
        {
            let min_bond = self
                .minimum_bond
                .get(state)
                .unwrap_infallible()
                .expect("The minimum bond should be set at genesis");

            let min_bond_value = min_bond.value(gas_price);

            if sequencer.balance < min_bond_value {
                return Err(AllowedSequencerError::InsufficientStakeAmount {
                    bond_amount: sequencer.balance,
                    minimum_bond_amount: min_bond_value,
                });
            }

            return Ok(sequencer);
        }

        Err(AllowedSequencerError::NotRegistered)
    }

    /// Returns the balance of the provided sender, if present.
    pub fn get_sender_balance<Reader: StateReader<User>>(
        &self,
        sender: &Da::Address,
        state: &mut Reader,
    ) -> Result<Option<Amount>, Reader::Error> {
        Ok(self
            .allowed_sequencers
            .get(sender, state)?
            .map(|s| s.balance))
    }

    /// Returns the rollup address of the sequencer with the given DA address.
    pub fn get_sequencer_address<Reader: StateReader<User>>(
        &self,
        da_address: Da::Address,
        state_accessor: &mut Reader,
    ) -> Result<Option<S::Address>, Reader::Error> {
        Ok(self
            .allowed_sequencers
            .get(&da_address, state_accessor)?
            .map(|s| s.address))
    }

    /// Slash the sequencer with the given address.
    pub fn slash_sequencer(
        &self,
        da_address: &Da::Address,
        state: &mut impl InfallibleStateAccessor,
    ) {
        self.delete_allowed_staker(da_address, state)
            .unwrap_infallible();
    }

    /// Check if the provided `Da::Address` belongs to a registered sequencer.
    pub fn is_registered_sequencer<Reader: StateReader<User>>(
        &self,
        da_address: &Da::Address,
        state: &mut Reader,
    ) -> Result<bool, Reader::Error> {
        Ok(self.allowed_sequencers.get(da_address, state)?.is_some())
    }

    /// Rewards the sequencer with the `amount` of gas tokens.
    /// Transfers the reward from the module's account to the sequencer's account.
    ///
    /// # Safety note:
    /// This method panics if the module account does not have enough funds to pay for the reward (the module balance should be populated in the `GasEnforcer` capability hook).
    pub fn reward_sequencer(
        &self,
        sequencer: &S::Address,
        amount: u64,
        state: &mut TxScratchpad<S::Storage>,
    ) {
        self.bank
            .transfer_from(
                self.bank.id().to_payable(),
                sequencer,
                Coins {
                    amount,
                    token_id: GAS_TOKEN_ID,
                },
                state,
            )
            .expect(
                "Impossible to transfer the reward from the module account to the sequencer. This is a bug",
            );
    }
}
