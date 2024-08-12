use std::fmt::Debug;

use anyhow::Result;
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sov_bank::{BurnRate, Coins, IntoPayable, GAS_TOKEN_ID};
use sov_modules_api::macros::{config_value, UniversalWallet};
use sov_modules_api::registration_lib::RegistrationError;
use sov_modules_api::{
    CallResponse, DaSpec, EventEmitter, ModuleInfo, Spec, StateAccessor, StateReader, TxState,
};
use sov_state::{EventContainer, User};
use thiserror::Error;

use crate::{Event, ProverIncentives};

/// This enumeration represents the available call messages for interacting with the `ExampleModule` module.
#[cfg_attr(feature = "native", derive(schemars::JsonSchema))]
#[derive(
    Serialize, Deserialize, BorshDeserialize, BorshSerialize, Debug, PartialEq, UniversalWallet,
)]
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

/// The prover incentives module does not have a custom error. Therefore, this enum is non instantiable type
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CustomError {}

#[allow(type_alias_bounds)]
type ProverRegistryError<S: Spec, ST: StateAccessor> =
    RegistrationError<S::Address, S::Address, <ST as StateReader<User>>::Error, CustomError>;

impl<S: Spec, Da: DaSpec> ProverIncentives<S, Da> {
    /// The burn rate of the reward price for the provers.
    /// The burn rate is a percentage of the base fee that is burned - this prevents provers from proving empty blocks.
    pub(crate) const fn burn_rate(&self) -> BurnRate {
        const PERCENT_BASE_FEE_TO_BURN: u8 = config_value!("PERCENT_BASE_FEE_TO_BURN");

        BurnRate::new_unchecked(PERCENT_BASE_FEE_TO_BURN)
    }
    /// A helper function for the `register` call. Also used to bond provers
    /// during genesis when no context is available.
    pub(super) fn register_prover<ST: StateAccessor + EventContainer>(
        &self,
        bond_amount: u64,
        prover: &S::Address,
        state: &mut ST,
    ) -> Result<CallResponse, ProverRegistryError<S, ST>> {
        if self.bonded_provers.get(prover, state)?.is_some() {
            return Err(RegistrationError::AlreadyRegistered(prover.clone()));
        }

        let minimum_bond = self
            .minimum_bond
            .get(state)?
            .ok_or(RegistrationError::NoMinimumBondSet(prover.clone()))?;

        if bond_amount < minimum_bond {
            return Err(RegistrationError::InsufficientStakeAmount {
                address: prover.clone(),
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
            .map_err(|_| RegistrationError::InsufficientFundsToRegister {
                address: prover.clone(),
                amount: bond_amount,
            })?;

        self.bonded_provers.set(prover, &bond_amount, state)?;

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
    pub(crate) fn register<ST: TxState<S>>(
        &self,
        bond_amount: u64,
        prover_address: &S::Address,
        state: &mut ST,
    ) -> Result<CallResponse, ProverRegistryError<S, ST>> {
        self.register_prover(bond_amount, prover_address, state)
    }

    /// Increases the balance of the provided sender, updating the state of the bonded provers.
    pub(crate) fn deposit<ST: TxState<S>>(
        &self,
        amount: u64,
        prover_address: &S::Address,
        state: &mut ST,
    ) -> Result<CallResponse, ProverRegistryError<S, ST>> {
        let bonded_amount = self
            .bonded_provers
            .get(prover_address, state)?
            .ok_or(RegistrationError::IsNotRegistered(prover_address.clone()))?;

        let balance = bonded_amount.checked_add(amount).ok_or(
            RegistrationError::ToppingAccountMakesBalanceOverflow {
                address: prover_address.clone(),
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
            .map_err(|_| RegistrationError::InsufficientFundsToTopUpAccount {
                address: prover_address.clone(),
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
    pub(crate) fn exit<ST: TxState<S>>(
        &self,
        prover_address: &S::Address,
        state: &mut ST,
    ) -> Result<CallResponse, ProverRegistryError<S, ST>> {
        // Get the prover's old balance.
        if let Some(old_balance) = self.bonded_provers.get(prover_address, state)? {
            let coins = Coins {
                token_id: GAS_TOKEN_ID,
                amount: old_balance,
            };

            self.bank
                .transfer_from(self.id.to_payable(), prover_address, coins, state)
                .map_err(
                    |_err| RegistrationError::InsufficientFundsToRefundStakedAmount {
                        address: prover_address.clone(),
                        amount: old_balance,
                    },
                )?;

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
