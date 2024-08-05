use std::fmt::Debug;

use anyhow::Result;
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_bank::{BurnRate, Coins, IntoPayable, GAS_TOKEN_ID};
use sov_modules_api::macros::config_value;
use sov_modules_api::{
    CallResponse, DaSpec, EventEmitter, Gas, ModuleInfo, Spec, StateAccessor, StateAccessorError,
    TxState,
};
use sov_state::EventContainer;
use thiserror::Error;

use crate::{Event, ProverIncentives};

/// This enumeration represents the available call messages for interacting with the `ExampleModule` module.
#[cfg_attr(feature = "native", derive(schemars::JsonSchema))]
#[derive(Serialize, Deserialize, BorshDeserialize, BorshSerialize, Debug, PartialEq)]
// TODO: allow call messages to borrow data
//     https://github.com/Sovereign-Labs/sovereign-sdk/issues/274
pub enum CallMessage {
    /// Add a new prover as a bonded prover.
    Register(u64),
    /// Increases the balance of the prover, transferring the funds from the prover account
    /// to the rollup.
    Deposit(u64),
    /// Unbonds the prover.
    Exit,
}

/// Error raised while processing the attester incentives
#[derive(Debug, Error, PartialEq)]
pub enum ProverIncentiveError {
    #[error("Stake amount below the minimum needed to register a prover")]
    /// Stake amount below the minimum needed to register a prover.
    InsufficientStakeAmount {
        /// The amount of gas tokens the sender is trying to stake.
        bond_amount: u64,
        /// The minimum amount of gas tokens to stake.
        minimum_bond_amount: u64,
    },

    #[error(
        "The minimum bond is not set. This is a bug - the minimum bond should be set at genesis"
    )]
    /// The minimum bond is not set. This is a bug - the minimum bond should be set at genesis
    NoMinimumBondSet,

    #[error("Insufficient funds on the prover's account to top up it's staked balance")]
    /// Insufficient funds on the prover's account to top up it's staked balance
    InsufficientFundsToTopUpAccount {
        /// The amount to add to the balance of the prover's account.
        amount_to_add: u64,
    },

    #[error("The provided amount makes the balance of the prover's account overflow.")]
    /// The provided amount makes the balance of the provers's account overflow.
    ToppingAccountMakesBalanceOverflow {
        /// The existing staked balance of the prover's account.
        existing_balance: u64,
        /// The amount to add to the balance of the prover's account.
        amount_to_add: u64,
    },

    #[error("The provided address is not an allowed sequencer")]
    /// The prover is not registered.
    IsNotRegisteredProver,

    #[error("The prover is already registered")]
    /// The prover is already registered.
    ProverAlreadyRegistered,
    #[error("Error occurred when transferring funds to bond the prover. The prover's account may not have enough funds")]
    /// An error occurred when transferring funds to bond the prover
    BondTransferFailure,

    #[error("Error occurred when transferring funds to unbond or reward the prover. This module's account may not have enough funds.
    This is a bug. Error: {0}")]
    /// An error occurred when trying to mint the reward token
    TransferFailure(String),

    /// An error when total bond value overflow or underflow
    #[error("Error when trying to top up bonded amount and it overflow or underflow")]
    BondArithmeticsError,

    /// An error when trying to access the state
    #[error("An error occurred when trying to access the state, error: {0}")]
    StateAccessorError(String),
}

impl<GU: Gas> From<StateAccessorError<GU>> for ProverIncentiveError {
    fn from(value: StateAccessorError<GU>) -> Self {
        ProverIncentiveError::StateAccessorError(value.to_string())
    }
}

impl<S: Spec, Da: DaSpec> ProverIncentives<S, Da> {
    /// The burn rate of the reward price for the provers.
    /// The burn rate is a percentage of the base fee that is burned - this prevents provers from proving empty blocks.
    pub(crate) const fn burn_rate(&self) -> BurnRate {
        const PERCENT_BASE_FEE_TO_BURN: u8 = config_value!("PERCENT_BASE_FEE_TO_BURN");

        BurnRate::new_unchecked(PERCENT_BASE_FEE_TO_BURN)
    }
    /// A helper function for the `register` call. Also used to bond provers
    /// during genesis when no context is available.
    pub(super) fn register_prover(
        &self,
        bond_amount: u64,
        prover: &S::Address,
        state: &mut (impl StateAccessor + EventContainer),
    ) -> Result<CallResponse, ProverIncentiveError> {
        if self
            .bonded_provers
            .get(prover, state)
            .map_err(|e| ProverIncentiveError::StateAccessorError(e.to_string()))?
            .is_some()
        {
            return Err(ProverIncentiveError::ProverAlreadyRegistered);
        }

        let minimum_bond = self
            .minimum_bond
            .get(state)
            .map_err(|e| ProverIncentiveError::StateAccessorError(e.to_string()))?
            .ok_or(ProverIncentiveError::NoMinimumBondSet)?;

        if bond_amount < minimum_bond {
            return Err(ProverIncentiveError::InsufficientStakeAmount {
                bond_amount,
                minimum_bond_amount: minimum_bond,
            });
        }

        // Transfer the bond amount from the sender to the module's id.
        // On failure, no state is changed
        let coins = Coins {
            token_id: GAS_TOKEN_ID,
            amount: bond_amount,
        };
        self.bank
            .transfer_from(prover, self.id.to_payable(), coins, state)
            .map_err(|_| ProverIncentiveError::BondTransferFailure)?;

        self.bonded_provers
            .set(prover, &bond_amount, state)
            .map_err(|e| ProverIncentiveError::StateAccessorError(e.to_string()))?;

        // Emit the bonding event
        self.emit_event(
            state,
            Event::<S>::Registered {
                prover: prover.clone(),
                amount: bond_amount,
            },
        );

        Ok(CallResponse::default())
    }

    /// Try to bond the requested amount of coins from context.sender()
    pub(crate) fn register(
        &self,
        bond_amount: u64,
        prover_address: &S::Address,
        state: &mut impl TxState<S>,
    ) -> Result<CallResponse, ProverIncentiveError> {
        self.register_prover(bond_amount, prover_address, state)
    }

    /// Increases the balance of the provided sender, updating the state of the bonded provers.
    pub(crate) fn deposit(
        &self,
        amount: u64,
        prover_address: &S::Address,
        state: &mut impl TxState<S>,
    ) -> Result<CallResponse, ProverIncentiveError> {
        let bonded_amount = self
            .bonded_provers
            .get(prover_address, state)?
            .ok_or(ProverIncentiveError::IsNotRegisteredProver)?;

        let balance = bonded_amount.checked_add(amount).ok_or(
            ProverIncentiveError::ToppingAccountMakesBalanceOverflow {
                existing_balance: bonded_amount,
                amount_to_add: amount,
            },
        )?;

        let coins = Coins {
            amount,
            token_id: GAS_TOKEN_ID,
        };

        self.bank
            .transfer_from(prover_address, self.id().to_payable(), coins, state)
            .map_err(|_| ProverIncentiveError::InsufficientFundsToTopUpAccount {
                amount_to_add: amount,
            })?;

        self.bonded_provers.set(prover_address, &balance, state)?;

        self.emit_event(
            state,
            Event::<S>::Deposited {
                prover: prover_address.clone(),
                deposit: amount,
            },
        );

        Ok(CallResponse::default())
    }

    /// Try to unbond the requested amount of coins with context.sender() as the beneficiary.
    pub(crate) fn exit(
        &self,
        prover_address: &S::Address,
        state: &mut impl TxState<S>,
    ) -> Result<CallResponse, ProverIncentiveError> {
        // Get the prover's old balance.
        if let Some(old_balance) = self.bonded_provers.get(prover_address, state)? {
            let coins = Coins {
                token_id: GAS_TOKEN_ID,
                amount: old_balance,
            };

            self.bank
                .transfer_from(self.id.to_payable(), prover_address, coins, state)
                .map_err(|err| ProverIncentiveError::TransferFailure(err.to_string()))?;

            // Update our internal tracking of the total bonded amount for the sender.
            self.bonded_provers.set(prover_address, &0, state)?;

            // Emit the unbonding event
            self.emit_event(
                state,
                Event::<S>::Exited {
                    prover: prover_address.clone(),
                    amount_withdrawn: old_balance,
                },
            );
        }

        Ok(CallResponse::default())
    }
}
