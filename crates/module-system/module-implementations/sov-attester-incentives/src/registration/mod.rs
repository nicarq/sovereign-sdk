pub(crate) mod attester;
pub(crate) mod challenger;

use core::result::Result::Ok;

use sov_bank::{Amount, Coins, IntoPayable, GAS_TOKEN_ID};
use sov_modules_api::registration_lib::RegistrationError;
use sov_modules_api::{BasicAddress, Spec, StateAccessor, StateReader};
use sov_state::{EventContainer, User};
use thiserror::Error;

use crate::AttesterIncentives;

/// Custom errors for the attester incentives module
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CustomError<RollupAddress: BasicAddress> {
    #[error("Attester is unbonding")]
    /// The attester is in the first unbonding phase.
    AttesterIsUnbonding(RollupAddress),

    #[error("The first phase of unbonding has not been finalized")]
    /// The attester is trying to finish the two-phase unbonding too soon.
    UnbondingNotFinalized(RollupAddress),

    #[error("User is not trying to unbond at the time of the transaction")]
    /// User is not trying to unbond at the time of the transaction.
    AttesterIsNotUnbonding(RollupAddress),
}

#[allow(type_alias_bounds)]
type AttesterRegistryError<S: Spec, ST: StateAccessor> = RegistrationError<
    S::Address,
    S::Address,
    <ST as StateReader<User>>::Error,
    CustomError<S::Address>,
>;

impl<S, Da> AttesterIncentives<S, Da>
where
    S: sov_modules_api::Spec,
    Da: sov_modules_api::DaSpec,
{
    fn register_user_helper<ST: StateAccessor + EventContainer>(
        &self,
        bond_amount: u64,
        user_address: &S::Address,
        balances: &sov_modules_api::StateMap<S::Address, Amount>,
        state: &mut ST,
    ) -> Result<(), AttesterRegistryError<S, ST>> {
        // Transfer the bond amount from the sender to the module's id.
        // On failure, no state is changed
        let coins = Coins {
            token_id: GAS_TOKEN_ID,
            amount: bond_amount,
        };

        self.bank
            .transfer_from(user_address, self.id.to_payable(), coins, state)
            .map_err(|_err| RegistrationError::InsufficientFundsToRegister {
                address: user_address.clone(),
                amount: bond_amount,
            })?;

        balances.set(user_address, &bond_amount, state)?;

        Ok(())
    }
}
