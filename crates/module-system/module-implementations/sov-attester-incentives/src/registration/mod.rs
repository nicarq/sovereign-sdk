pub(crate) mod attester;
pub(crate) mod challenger;
use core::result::Result::Ok;
use std::marker::PhantomData;

use sov_bank::{Amount, Coins, IntoPayable, GAS_TOKEN_ID};
use sov_modules_api::registration_lib::{RegistrationError, StakeRegistration};
use sov_modules_api::{BasicAddress, DaSpec, ModuleId, Spec, StateAccessor, StateReader};
use sov_state::User;
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

struct Staker<'a, S: Spec, Da: DaSpec> {
    bonded_stakers: &'a sov_modules_api::StateMap<S::Address, Amount>,
    minimum_bond: &'a sov_modules_api::StateValue<Amount>,
    bank: &'a sov_bank::Bank<S>,
    id: &'a ModuleId,
    _phantom: PhantomData<Da>,
}

impl<'a, S: Spec, Da: DaSpec> Staker<'a, S, Da> {
    fn new_challenger(attester_incentives: &'a AttesterIncentives<S, Da>) -> Self {
        Self {
            bonded_stakers: &attester_incentives.bonded_challengers,
            minimum_bond: &attester_incentives.minimum_challenger_bond,
            bank: &attester_incentives.bank,
            id: &attester_incentives.id,
            _phantom: PhantomData,
        }
    }

    fn new_attester(attester_incentives: &'a AttesterIncentives<S, Da>) -> Self {
        Self {
            bonded_stakers: &attester_incentives.bonded_attesters,
            minimum_bond: &attester_incentives.minimum_attester_bond,
            bank: &attester_incentives.bank,
            id: &attester_incentives.id,
            _phantom: PhantomData,
        }
    }
}

impl<'a, S, Da> StakeRegistration for Staker<'a, S, Da>
where
    S: Spec,
    Da: DaSpec,
{
    type PrimaryAddress = S::Address;

    type RollupAddress = S::Address;

    type CustomError = CustomError<S::Address>;

    fn get_minimum_bond<ST: StateAccessor>(
        &self,
        state: &mut ST,
    ) -> Result<Option<u64>, <ST as sov_modules_api::StateWriter<sov_state::User>>::Error> {
        self.minimum_bond.get(state)
    }

    fn get_allowed_staker<ST: StateAccessor>(
        &self,
        address: &Self::PrimaryAddress,
        state: &mut ST,
    ) -> Result<
        Option<(Self::RollupAddress, u64)>,
        <ST as sov_modules_api::StateWriter<sov_state::User>>::Error,
    > {
        self.bonded_stakers
            .get(address, state)
            .map(|opt| opt.map(|bond| (address.clone(), bond)))
    }

    fn set_allowed_staker<ST: StateAccessor>(
        &self,
        _primary_address: &Self::PrimaryAddress,
        rollup_address: &Self::RollupAddress,
        amount: u64,
        state: &mut ST,
    ) -> Result<(), <ST as sov_modules_api::StateWriter<sov_state::User>>::Error> {
        self.bonded_stakers.set(rollup_address, &amount, state)?;
        Ok(())
    }

    fn transfer_bond_from_staker<ST: StateAccessor>(
        &self,
        address: &Self::RollupAddress,
        amount: u64,
        state: &mut ST,
    ) -> anyhow::Result<()> {
        self.bank
            .transfer_from(address, self.id.to_payable(), gas_coins(amount), state)?;
        Ok(())
    }

    fn transfer_bond_to_staker<ST: StateAccessor>(
        &self,
        address: &Self::RollupAddress,
        amount: u64,
        state: &mut ST,
    ) -> anyhow::Result<()> {
        self.bank
            .transfer_from(self.id.to_payable(), address, gas_coins(amount), state)?;
        Ok(())
    }

    fn delete_allowed_staker<ST: StateAccessor>(
        &self,
        address: &Self::PrimaryAddress,
        state: &mut ST,
    ) -> Result<(), <ST as sov_modules_api::StateWriter<sov_state::User>>::Error> {
        self.bonded_stakers.delete(address, state)
    }
}

pub(crate) fn gas_coins(amount: u64) -> Coins {
    Coins {
        amount,
        token_id: GAS_TOKEN_ID,
    }
}
