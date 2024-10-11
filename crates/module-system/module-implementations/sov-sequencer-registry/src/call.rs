use sov_bank::Amount;
#[cfg(feature = "native")]
use sov_modules_api::macros::CliWalletArg;
#[cfg(feature = "native")]
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::registration_lib::{RegistrationError, StakeRegistration};
use sov_modules_api::{CallResponse, Context, DaSpec, EventEmitter, Spec, TxState};

use crate::{CustomError, Event, SequencerRegistry, SequencerRegistryError};

/// This enumeration represents the available call messages for interacting with
/// the `sov-sequencer-registry` module.
#[cfg_attr(
    feature = "native",
    derive(schemars::JsonSchema),
    derive(CliWalletArg, UniversalWallet)
)]
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
#[serde(rename_all = "snake_case")]
pub enum CallMessage {
    /// Add a new sequencer to the sequencer registry.
    Register {
        /// The raw Da address of the sequencer you're registering.
        da_address: Vec<u8>,
        /// The initial balance of the sequencer.
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

impl<S: Spec> SequencerRegistry<S> {
    /// Tries to register a sequencer by staking the provided amount of gas tokens.
    /// This method uses the context's sender as the sequencer's address.
    ///
    /// # Errors
    /// Will error
    ///
    /// - If the provided amount is below the minimum required to register a sequencer.
    /// - If the minimum bond is not set.
    /// - If the sender's account does not have enough funds to register itself as a sequencer.
    /// - If the sequencer is already registered.
    pub(crate) fn register<ST: TxState<S>>(
        &self,
        da_address: &<S::Da as DaSpec>::Address,
        amount: Amount,
        context: &Context<S>,
        state: &mut ST,
    ) -> Result<CallResponse, SequencerRegistryError<S, ST>> {
        let sequencer = context.sender();
        self.register_staker(da_address, sequencer, amount, state)?;

        self.emit_event(
            state,
            Event::<S>::Registered {
                sequencer: sequencer.clone(),
                amount,
            },
        );

        Ok(CallResponse::default())
    }

    pub(crate) fn deposit<ST: TxState<S>>(
        &self,
        da_address: &<S::Da as DaSpec>::Address,
        amount: u64,
        context: &Context<S>,
        state: &mut ST,
    ) -> Result<CallResponse, SequencerRegistryError<S, ST>> {
        let sender = context.sender();
        self.validate_sender(da_address, sender, state)?;

        self.deposit_funds(da_address, amount, state)?;

        self.emit_event(
            state,
            Event::<S>::Deposited {
                sequencer: sender.clone(),
                amount,
            },
        );

        Ok(CallResponse::default())
    }

    /// Tries to remove a sequencer by unstaking the provided amount of gas tokens.
    /// This method uses the context's sender as the sequencer's address.
    ///
    /// # Errors
    /// Will error
    ///
    /// - If the sequencer is not registered.
    /// - If the sequencer tries to unregister itself during the execution of its own batch.
    /// - If the supplied `da_address` does not match the transaction sender.
    /// - If the module balance is not high enough to refund the sequencer's staked amount (this is a bug).
    pub(crate) fn exit<ST: TxState<S>>(
        &self,
        da_address: &<S::Da as DaSpec>::Address,
        context: &Context<S>,
        state: &mut ST,
    ) -> Result<CallResponse, SequencerRegistryError<S, ST>> {
        let sender = context.sender();
        self.validate_sender(da_address, sender, state)?;

        if sender == context.sequencer() {
            return Err(RegistrationError::Custom(
                CustomError::CannotUnregisterDuringOwnBatch(da_address.clone()),
            ));
        }

        let amount_withdrawn = self.exit_staker(da_address, state)?;

        self.emit_event(
            state,
            Event::<S>::Exited {
                sequencer: sender.clone(),
                amount_withdrawn,
            },
        );
        Ok(CallResponse::default())
    }

    fn validate_sender<ST: TxState<S>>(
        &self,
        da_address: &<S::Da as DaSpec>::Address,
        sender: &S::Address,
        state: &mut ST,
    ) -> Result<(), SequencerRegistryError<S, ST>> {
        let belongs_to = self
            .allowed_sequencers
            .get_or_err(da_address, state)?
            .map_err(|_| RegistrationError::IsNotRegistered(da_address.clone()))?
            .address;

        if sender != &belongs_to {
            return Err(RegistrationError::Custom(
                CustomError::SuppliedAddressDoesNotMatchTxSender {
                    parameter: belongs_to,
                    sender: sender.clone(),
                },
            ));
        }

        Ok(())
    }
}
