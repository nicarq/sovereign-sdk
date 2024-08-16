mod errors;
pub use errors::*;
use sov_rollup_interface::{BasicAddress, RollupAddress as SovRollupAddress};
use sov_state::{EventContainer, User};

use crate::{StateAccessor, StateWriter};

/// A trait that abstracts the generic logic for staking and un-staking across various sov-modules.
pub trait StakeRegistration {
    /// The primary address, which can be either the DA address or the Rollup address, depending on the use case.
    type PrimaryAddress: BasicAddress;
    /// Address on the rollup.
    type RollupAddress: SovRollupAddress;
    /// Custom module error type. This allows handling module specific errors in the library logic.
    type CustomError;

    /// Tries to register a staker by staking the provided amount.
    #[allow(clippy::type_complexity)]
    fn register_staker<ST: StateAccessor + EventContainer>(
        &self,
        primary_address: &Self::PrimaryAddress,
        rollup_address: &Self::RollupAddress,
        amount: u64,
        state: &mut ST,
    ) -> Result<
        (),
        RegistrationError<
            Self::RollupAddress,
            Self::PrimaryAddress,
            <ST as StateWriter<User>>::Error,
            Self::CustomError,
        >,
    > {
        if self.get_allowed_staker(primary_address, state)?.is_some() {
            tracing::error!(staker = ?primary_address, "Staker already registered");
            return Err(RegistrationError::AlreadyRegistered(rollup_address.clone()));
        }

        let minimum_bond = match self.get_minimum_bond(state)? {
            Some(min_amount) => min_amount,
            None => {
                tracing::error!(staker = ?primary_address, "No minimum bond set");
                return Err(RegistrationError::NoMinimumBondSet(rollup_address.clone()));
            }
        };

        if amount < minimum_bond {
            tracing::error!(amount = ?amount, minimum_bond = ?minimum_bond, "Insufficient stake amount");
            return Err(RegistrationError::InsufficientStakeAmount {
                address: rollup_address.clone(),
                bond_amount: amount,
                minimum_bond_amount: minimum_bond,
            });
        }

        self.transfer_bond_from_staker(rollup_address, amount, state)
            .map_err(|e| {
                tracing::error!(staker = ?primary_address, error = ?e, "Insufficient funds to register");
                RegistrationError::InsufficientFundsToRegister {
                    address: rollup_address.clone(),
                    amount,
                }
            })?;

        self.set_allowed_staker(primary_address, rollup_address, amount, state)?;

        Ok(())
    }

    /// Increases the balance of the sender, updating the state of the registry.
    #[allow(clippy::type_complexity)]
    fn deposit_funds<ST: StateAccessor + EventContainer>(
        &self,
        staker: &Self::PrimaryAddress,
        amount: u64,
        state: &mut ST,
    ) -> Result<
        (),
        RegistrationError<
            Self::RollupAddress,
            Self::PrimaryAddress,
            <ST as StateWriter<User>>::Error,
            Self::CustomError,
        >,
    > {
        let (address, balance) = self.get_allowed_staker(staker, state)?.ok_or_else(|| {
            tracing::error!("Staker not registered");
            RegistrationError::IsNotRegistered(staker.clone())
        })?;

        let balance =  balance.checked_add(amount).ok_or_else(|| {
                tracing::error!(staker = ?staker, amount = ?amount, balance = ?balance, "Topping account makes balance overflow");
                RegistrationError::ToppingAccountMakesBalanceOverflow {
                    address: address.clone(),
                    existing_balance: balance,
                    amount_to_add: amount,
                }
        })?;

        self.transfer_bond_from_staker(&address, amount, state)
            .map_err(|e| {
                tracing::error!(staker = ?staker, error = ?e, "Insufficient funds to top up account");
                RegistrationError::InsufficientFundsToTopUpAccount {
                    address: address.clone(),
                    amount_to_add: amount,
                }
            })?;

        self.set_allowed_staker(staker, &address, balance, state)?;

        Ok(())
    }

    /// Tries to unstake the sender.
    #[allow(clippy::type_complexity)]
    fn exit_staker<ST: StateAccessor + EventContainer>(
        &self,
        staker: &Self::PrimaryAddress,
        state: &mut ST,
    ) -> Result<
        u64,
        RegistrationError<
            Self::RollupAddress,
            Self::PrimaryAddress,
            <ST as StateWriter<User>>::Error,
            Self::CustomError,
        >,
    > {
        let (address, balance) = self.get_allowed_staker(staker, state)?.ok_or_else(|| {
            tracing::error!(staker = ?staker, "Staker not registered");
            RegistrationError::IsNotRegistered(staker.clone())
        })?;

        self.transfer_bond_to_staker(&address, balance, state)
            .map_err(|e| {
                tracing::error!(staker = ?staker, error = ?e, "Insufficient funds to refund stake");
                RegistrationError::InsufficientFundsToRefundStakedAmount {
                    address: address.clone(),
                    amount: balance,
                }
            })?;

        self.delete_allowed_staker(staker, state)?;

        Ok(balance)
    }

    /// The minimum allowed bond.
    fn get_minimum_bond<ST: StateAccessor>(
        &self,
        state: &mut ST,
    ) -> Result<Option<u64>, <ST as StateWriter<User>>::Error>;

    /// Get the allowed staker.
    #[allow(clippy::type_complexity)]
    fn get_allowed_staker<ST: StateAccessor>(
        &self,
        address: &Self::PrimaryAddress,
        state: &mut ST,
    ) -> Result<Option<(Self::RollupAddress, u64)>, <ST as StateWriter<User>>::Error>;

    /// Set the allowed staker.
    fn set_allowed_staker<ST: StateAccessor>(
        &self,
        primary_address: &Self::PrimaryAddress,
        rollup_address: &Self::RollupAddress,
        amount: u64,
        state: &mut ST,
    ) -> Result<(), <ST as StateWriter<User>>::Error>;

    /// Transfer bond from a staker to the rollup.
    fn transfer_bond_from_staker<ST: StateAccessor>(
        &self,
        address: &Self::RollupAddress,
        amount: u64,
        state: &mut ST,
    ) -> Result<(), anyhow::Error>;

    /// Transfer bond from the rollup to a staker.
    fn transfer_bond_to_staker<ST: StateAccessor>(
        &self,
        address: &Self::RollupAddress,
        amount: u64,
        state: &mut ST,
    ) -> Result<(), anyhow::Error>;

    /// Delete the allowed staker.
    fn delete_allowed_staker<ST: StateAccessor>(
        &self,
        address: &Self::PrimaryAddress,
        state: &mut ST,
    ) -> Result<(), <ST as StateWriter<User>>::Error>;
}
