use std::fmt::Debug;

use anyhow::{Context as AnyhowContext, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_bank::{BurnRate, Coins, IntoPayable, GAS_TOKEN_ID};
use sov_modules_api::macros::config_value;
use sov_modules_api::{
    CallResponse, DaSpec, EventEmitter, Gas, Spec, StateAccessor, StateAccessorError, TxState,
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
    /// Bonds the prover with provided bond.
    BondProver(u64),
    /// Unbonds the prover.
    UnbondProver,
}

/// Error raised while processing the attester incentives
#[derive(Debug, Error, PartialEq)]
pub enum ProverIncentiveError {
    #[error("The bond is not high enough")]
    /// The bond is below the minimum bond
    BondNotHighEnough,

    #[error("Prover is not bonded at the time of the transaction")]
    /// User is not bonded at the time of the transaction
    ProverNotBonded,

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
    /// A helper function for the `bond_prover` call. Also used to bond provers
    /// during genesis when no context is available.
    pub(super) fn bond_prover_helper(
        &self,
        bond_amount: u64,
        prover: &S::Address,
        state: &mut (impl StateAccessor + EventContainer),
    ) -> Result<CallResponse, ProverIncentiveError> {
        // Transfer the bond amount from the sender to the module's id.
        // On failure, no state is changed
        let coins = Coins {
            token_id: GAS_TOKEN_ID,
            amount: bond_amount,
        };
        self.bank
            .transfer_from(prover, self.id.to_payable(), coins, state)
            .map_err(|_| ProverIncentiveError::BondTransferFailure)?;

        // Check that total balance does not overflow before doing transfer.
        let old_balance = self
            .bonded_provers
            .get(prover, state)
            .map_err(|e| ProverIncentiveError::StateAccessorError(e.to_string()))?
            .unwrap_or_default();

        let total_balance = old_balance
            .checked_add(bond_amount)
            .with_context(|| {
                anyhow::anyhow!("The total balance overflows with the given operation")
            })
            .map_err(|_e| ProverIncentiveError::BondArithmeticsError)?;

        // Update our record of the total bonded amount for the sender.
        // This update is infallible, so no value can be destroyed.
        self.bonded_provers
            .set(prover, &total_balance, state)
            .map_err(|e| ProverIncentiveError::StateAccessorError(e.to_string()))?;

        // Emit the bonding event
        self.emit_event(
            state,
            Event::<S>::BondedProver {
                prover: prover.clone(),
                deposit: bond_amount,
                total_balance,
            },
        );

        Ok(CallResponse::default())
    }

    /// Try to bond the requested amount of coins from context.sender()
    pub(crate) fn bond_prover(
        &self,
        bond_amount: u64,
        prover_address: &S::Address,
        state: &mut impl TxState<S>,
    ) -> Result<CallResponse, ProverIncentiveError> {
        self.bond_prover_helper(bond_amount, prover_address, state)
    }

    /// Try to unbond the requested amount of coins with context.sender() as the beneficiary.
    pub(crate) fn unbond_prover(
        &self,
        prover_address: &S::Address,
        state: &mut impl TxState<S>,
    ) -> Result<CallResponse, ProverIncentiveError> {
        // Get the prover's old balance.
        if let Some(old_balance) = self.bonded_provers.get(prover_address, state)? {
            self.transfer_to_prover(old_balance, prover_address, state)?;

            // Update our internal tracking of the total bonded amount for the sender.
            self.bonded_provers.set(prover_address, &0, state)?;

            // Emit the unbonding event
            self.emit_event(
                state,
                Event::<S>::UnBondedProver {
                    prover: prover_address.clone(),
                    amount_withdrawn: old_balance,
                },
            );
        }

        Ok(CallResponse::default())
    }

    /// Transfer the given amount of tokens to the prover
    pub(crate) fn transfer_to_prover(
        &self,
        total_reward: u64,
        sender: &S::Address,
        state: &mut impl StateAccessor,
    ) -> Result<(), ProverIncentiveError> {
        let coins = Coins {
            token_id: GAS_TOKEN_ID,
            amount: total_reward,
        };

        // We can transfer the reward from the `ProverIncentives` module to the prover's account.
        self.bank
            .transfer_from(self.id.to_payable(), sender, coins, state)
            .map_err(|err| ProverIncentiveError::TransferFailure(err.to_string()))?;

        Ok(())
    }
}
