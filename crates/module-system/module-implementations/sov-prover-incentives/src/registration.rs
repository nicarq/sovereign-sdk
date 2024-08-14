use anyhow::Result;
use sov_bank::{Coins, IntoPayable, GAS_TOKEN_ID};
use sov_modules_api::registration_lib::StakeRegistration;
use sov_modules_api::{DaSpec, EventEmitter, ModuleInfo, Spec, StateAccessor};
use sov_state::{EventContainer, User};

use crate::{CustomError, Event, ProverIncentives};

impl<S: Spec, Da: DaSpec> StakeRegistration for ProverIncentives<S, Da> {
    type PrimaryAddress = S::Address;

    type RollupAddress = S::Address;

    type CustomError = CustomError;

    fn get_minimum_bond<ST: StateAccessor>(
        &self,
        state: &mut ST,
    ) -> Result<Option<u64>, <ST as sov_modules_api::StateWriter<User>>::Error> {
        self.minimum_bond.get(state)
    }

    fn get_allowed_staker<ST: StateAccessor>(
        &self,
        address: &Self::PrimaryAddress,
        state: &mut ST,
    ) -> std::result::Result<
        Option<(Self::RollupAddress, u64)>,
        <ST as sov_modules_api::StateWriter<User>>::Error,
    > {
        self.bonded_provers
            .get(address, state)
            .map(|opt| opt.map(|b| (address.clone(), b)))
    }

    fn set_allowed_staker<ST: StateAccessor>(
        &self,
        primary_address: &Self::PrimaryAddress,
        _rollup_address: &Self::RollupAddress,
        amount: u64,
        state: &mut ST,
    ) -> Result<(), <ST as sov_modules_api::StateWriter<User>>::Error> {
        self.bonded_provers.set(primary_address, &amount, state)?;
        Ok(())
    }

    fn transfer_bond_from_staker<ST: StateAccessor>(
        &self,
        address: &Self::RollupAddress,
        amount: u64,
        state: &mut ST,
    ) -> Result<(), anyhow::Error> {
        self.bank
            .transfer_from(address, self.id().to_payable(), gas_coins(amount), state)?;
        Ok(())
    }

    fn transfer_bond_to_staker<ST: StateAccessor>(
        &self,
        address: &Self::RollupAddress,
        amount: u64,
        state: &mut ST,
    ) -> Result<(), anyhow::Error> {
        self.bank
            .transfer_from(self.id().to_payable(), address, gas_coins(amount), state)?;
        Ok(())
    }

    fn delete_allowed_staker<ST: StateAccessor>(
        &self,
        address: &Self::PrimaryAddress,
        state: &mut ST,
    ) -> Result<(), <ST as sov_modules_api::StateWriter<User>>::Error> {
        self.bonded_provers.delete(address, state)?;
        Ok(())
    }

    fn emit_registered<ST: StateAccessor + EventContainer>(
        &self,
        address: &Self::RollupAddress,
        amount: u64,
        state: &mut ST,
    ) {
        self.emit_event(
            state,
            Event::<S>::Registered {
                prover: address.clone(),
                amount,
            },
        );
    }

    fn emit_deposited<ST: StateAccessor + EventContainer>(
        &self,
        address: &Self::RollupAddress,
        amount: u64,
        state: &mut ST,
    ) {
        self.emit_event(
            state,
            Event::<S>::Deposited {
                prover: address.clone(),
                deposit: amount,
            },
        );
    }

    fn emit_exited<ST: StateAccessor + EventContainer>(
        &self,
        address: &Self::RollupAddress,
        amount_withdrawn: u64,
        state: &mut ST,
    ) {
        self.emit_event(
            state,
            Event::<S>::Exited {
                prover: address.clone(),
                amount_withdrawn,
            },
        );
    }
}

fn gas_coins(amount: u64) -> Coins {
    Coins {
        amount,
        token_id: GAS_TOKEN_ID,
    }
}
