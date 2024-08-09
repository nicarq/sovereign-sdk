use sov_bank::{Amount, Coins, IntoPayable, GAS_TOKEN_ID};
#[cfg(feature = "native")]
use sov_modules_api::macros::CliWalletArg;
use sov_modules_api::prelude::UnwrapInfallible;
use sov_modules_api::registration_lib::RegistrationError;
use sov_modules_api::{
    CallResponse, Context, EventEmitter, ModuleInfo, StateAccessor, StateCheckpoint, StateWriter,
    TxState,
};
use sov_state::User;

use crate::event::Event;
use crate::{AllowedSequencer, CustomError, SequencerRegistry, SequencerRegistryError};

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

impl<S: sov_modules_api::Spec, Da: sov_modules_api::DaSpec> SequencerRegistry<S, Da> {
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
        da_address: &Da::Address,
        amount: Amount,
        context: &Context<S>,
        state: &mut ST,
    ) -> Result<CallResponse, SequencerRegistryError<S, Da, ST>> {
        let sequencer = context.sender();
        self.register_sequencer(da_address, sequencer, amount, state)?;
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
        da_address: &Da::Address,
        context: &Context<S>,
        state: &mut ST,
    ) -> Result<CallResponse, SequencerRegistryError<S, Da, ST>> {
        let sender = context.sender();

        let belongs_to = self
            .allowed_sequencers
            .get_or_err(da_address, state)?
            .map_err(|_| RegistrationError::IsNotRegistered(da_address.clone()))?
            .address;

        if &belongs_to == context.sequencer() {
            return Err(RegistrationError::Custom(
                CustomError::CannotUnregisterDuringOwnBatch(da_address.clone()),
            ));
        }

        if sender != &belongs_to {
            return Err(RegistrationError::Custom(
                CustomError::SuppliedAddressDoesNotMatchTxSender {
                    parameter: belongs_to,
                    sender: sender.clone(),
                },
            ));
        }

        let sender_balance = self.get_sender_balance(da_address, state)?.unwrap_or(0);

        self.bank
            .transfer_from(
                self.id().to_payable(),
                sender,
                Coins {
                    amount: sender_balance,
                    token_id: GAS_TOKEN_ID,
                },
                state,
            )
            .map_err(
                |_| RegistrationError::InsufficientFundsToRefundStakedAmount {
                    address: sender.clone(),
                    amount: sender_balance,
                },
            )?;

        // we remove the sequencer from the registry *once the sequencer has received its staked amount*
        self.delete(da_address, state)?;

        self.emit_event(
            state,
            Event::<S>::Exited {
                sequencer: sender.clone(),
            },
        );

        Ok(CallResponse::default())
    }

    pub(crate) fn delete<Accessor: StateAccessor>(
        &self,
        da_address: &Da::Address,
        state: &mut Accessor,
    ) -> Result<(), <Accessor as StateWriter<User>>::Error> {
        self.allowed_sequencers.delete(da_address, state)?;

        if let Some(preferred_sequencer) = self.preferred_sequencer.get(state)? {
            if da_address == &preferred_sequencer {
                self.preferred_sequencer.delete(state)?;
            }
        }

        Ok(())
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
    pub(crate) fn deposit<ST: TxState<S>>(
        &self,
        sender: &Da::Address,
        amount: Amount,
        state: &mut ST,
    ) -> Result<CallResponse, SequencerRegistryError<S, Da, ST>> {
        let AllowedSequencer { address, balance } = self
            .allowed_sequencers
            .get(sender, state)?
            .ok_or(RegistrationError::IsNotRegistered(sender.clone()))?;

        let balance = balance.checked_add(amount).ok_or(
            RegistrationError::ToppingAccountMakesBalanceOverflow {
                address: address.clone(),
                existing_balance: balance,
                amount_to_add: amount,
            },
        )?;

        let coins = Coins {
            amount,
            token_id: GAS_TOKEN_ID,
        };

        self.bank
            .transfer_from(&address, self.id().to_payable(), coins, state)
            .map_err(|_| RegistrationError::InsufficientFundsToTopUpAccount {
                address: address.clone(),
                amount_to_add: amount,
            })?;

        self.allowed_sequencers.set(
            sender,
            &AllowedSequencer {
                address: address.clone(),
                balance,
            },
            state,
        )?;

        self.emit_event(
            state,
            Event::<S>::Deposited {
                sequencer: address,
                amount,
            },
        );

        Ok(CallResponse::default())
    }

    /// Rewards the sequencer with the `amount` of gas tokens.
    /// Transfers the reward from the module's account to the sequencer's account.
    ///
    /// # Safety note:
    /// This method panics if:
    /// - The sequencer is not registered.
    /// - The module account does not have enough funds to pay for the reward (the module balance should be populated in the `GasEnforcer` capability hook).
    pub fn reward_sequencer(
        &self,
        sequencer: &Da::Address,
        amount: u64,
        state: &mut StateCheckpoint<S>,
    ) {
        let AllowedSequencer {
            address: rollup_address,
            balance: _,
        } = self
            .allowed_sequencers
            .get(sequencer, state)
            .unwrap_infallible()
            .expect("Sequencer must be allowed.");

        self.bank
            .transfer_from(
                self.id().to_payable(),
                &rollup_address,
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
