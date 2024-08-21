use sov_bank::{Coins, IntoPayable, GAS_TOKEN_ID};
use sov_modules_api::registration_lib::StakeRegistration;
use sov_modules_api::{ModuleInfo, StateAccessor, StateWriter};
use sov_state::User;

use crate::{AllowedSequencer, CustomError, SequencerRegistry};

impl<S: sov_modules_api::Spec, Da: sov_modules_api::DaSpec> StakeRegistration
    for SequencerRegistry<S, Da>
{
    type PrimaryAddress = Da::Address;

    type RollupAddress = S::Address;

    type CustomError = CustomError<Self::RollupAddress, Self::PrimaryAddress>;

    fn get_minimum_bond<ST: StateAccessor>(
        &self,
        state: &mut ST,
    ) -> Result<Option<u64>, <ST as StateWriter<User>>::Error> {
        self.minimum_bond.get(state)
    }

    fn get_allowed_staker<ST: StateAccessor>(
        &self,
        address: &Self::PrimaryAddress,
        state: &mut ST,
    ) -> Result<Option<(Self::RollupAddress, u64)>, <ST as StateWriter<User>>::Error> {
        let res = self.allowed_sequencers.get(address, state)?;
        Ok(res.map(|s| (s.address, s.balance)))
    }

    fn set_allowed_staker<ST: StateAccessor>(
        &self,
        primary_address: &Self::PrimaryAddress,
        rollup_address: &Self::RollupAddress,
        amount: u64,
        state: &mut ST,
    ) -> Result<(), <ST as StateWriter<User>>::Error> {
        self.allowed_sequencers.set(
            primary_address,
            &AllowedSequencer {
                address: rollup_address.clone(),
                balance: amount,
            },
            state,
        )
    }

    fn transfer_bond_from_staker<ST: StateAccessor>(
        &self,
        address: &Self::RollupAddress,
        amount: u64,
        state: &mut ST,
    ) -> anyhow::Result<()> {
        self.bank
            .transfer_from(address, self.id().to_payable(), gas_coins(amount), state)?;

        Ok(())
    }

    fn transfer_bond_to_staker<ST: StateAccessor>(
        &self,
        address: &Self::RollupAddress,
        amount: u64,
        state: &mut ST,
    ) -> anyhow::Result<()> {
        self.bank
            .transfer_from(self.id().to_payable(), address, gas_coins(amount), state)?;

        Ok(())
    }

    fn delete_allowed_staker<ST: StateAccessor>(
        &self,
        da_address: &Self::PrimaryAddress,
        state: &mut ST,
    ) -> Result<(), <ST as StateWriter<User>>::Error> {
        self.allowed_sequencers.delete(da_address, state)?;

        if let Some(preferred_sequencer) = self.preferred_sequencer.get(state)? {
            if da_address == &preferred_sequencer {
                self.preferred_sequencer.delete(state)?;
            }
        }

        Ok(())
    }
}

fn gas_coins(amount: u64) -> Coins {
    Coins {
        amount,
        token_id: GAS_TOKEN_ID,
    }
}
