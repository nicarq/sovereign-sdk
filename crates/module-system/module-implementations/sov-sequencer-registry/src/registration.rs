use sov_bank::{config_gas_token_id, Coins, IntoPayable};
use sov_modules_api::registration_lib::StakeRegistration;
use sov_modules_api::{DaSpec, Gas, ModuleInfo, Spec, StateAccessor, StateWriter, TxState};
use sov_state::User;

use crate::{AllowedSequencer, CustomError, SequencerRegistry};

impl<S: Spec> StakeRegistration for SequencerRegistry<S> {
    type Spec = S;

    type PrimaryAddress = <S::Da as DaSpec>::Address;

    type RollupAddress = S::Address;

    type CustomError = CustomError<Self::RollupAddress, Self::PrimaryAddress>;

    fn get_minimum_bond<ST: TxState<S>>(
        &self,
        state: &mut ST,
    ) -> Result<Option<u64>, <ST as StateWriter<User>>::Error> {
        self.minimum_bond
            .get(state)
            .map(|maybe_bond| maybe_bond.map(|bond| bond.value(&state.gas_info().gas_price)))
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
        token_id: config_gas_token_id(),
    }
}
