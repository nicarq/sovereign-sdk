use core::result::Result::Ok;

use sov_bank::{Coins, IntoPayable, GAS_TOKEN_ID};
use sov_modules_api::{
    CallResponse, Context, EventEmitter, StateAccessor, StateAccessorError, StateReader, TxState,
};
use sov_state::{EventContainer, User};

use crate::{AttesterIncentiveErrors, AttesterIncentives, Event, UnbondingInfo};

impl<S, Da> AttesterIncentives<S, Da>
where
    S: sov_modules_api::Spec,
    Da: sov_modules_api::DaSpec,
{
    pub(crate) fn register_attester<ST: StateAccessor + EventContainer>(
        &self,
        bond_amount: u64,
        user_address: &S::Address,
        state: &mut ST,
    ) -> Result<CallResponse, AttesterIncentiveErrors<<ST as StateReader<User>>::Error>> {
        if self.unbonding_attesters.get(user_address, state)?.is_some() {
            return Err(AttesterIncentiveErrors::AttesterIsUnbonding);
        }

        if self.bonded_attesters.get(user_address, state)?.is_some() {
            return Err(AttesterIncentiveErrors::AlreadyRegistered);
        }

        let minimum_bond = self
            .minimum_attester_bond
            .get(state)?
            .ok_or(AttesterIncentiveErrors::NoMinimumBondSet)?;

        if bond_amount < minimum_bond {
            return Err(AttesterIncentiveErrors::InsufficientStakeAmount {
                bond_amount,
                minimum_bond_amount: minimum_bond,
            });
        }

        let balances = &self.bonded_attesters;
        self.register_user_helper::<ST>(bond_amount, user_address, balances, state)?;

        let event = Event::<S>::RegisteredAttester {
            amount: bond_amount,
        };

        self.emit_event(state, event);
        Ok(CallResponse::default())
    }

    pub(crate) fn deposit_attester<ST: StateAccessor + EventContainer>(
        &self,
        amount: u64,
        attester_address: &S::Address,
        state: &mut ST,
    ) -> Result<CallResponse, AttesterIncentiveErrors<<ST as StateReader<User>>::Error>> {
        if self
            .unbonding_attesters
            .get(attester_address, state)?
            .is_some()
        {
            return Err(AttesterIncentiveErrors::AttesterIsUnbonding);
        }

        let bonded_amount = self
            .bonded_attesters
            .get(attester_address, state)?
            .ok_or(AttesterIncentiveErrors::IsNotRegistered)?;

        let balance = bonded_amount.checked_add(amount).ok_or(
            AttesterIncentiveErrors::ToppingAccountMakesBalanceOverflow {
                existing_balance: bonded_amount,
                amount_to_add: amount,
            },
        )?;

        let coins = Coins {
            amount,
            token_id: GAS_TOKEN_ID,
        };

        self.bank
            .transfer_from(attester_address, self.id.to_payable(), coins, state)
            .map_err(|_err| AttesterIncentiveErrors::BondTransferFailure)?;

        self.bonded_attesters
            .set(attester_address, &balance, state)?;

        Ok(CallResponse::default())
    }

    /// The attester starts the first phase of the two-phase unbonding.
    /// We put the current max finalized height with the attester address
    /// in the set of unbonding attesters if the attester
    /// is already present in the unbonding set
    pub(crate) fn begin_exit_attester(
        &self,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<CallResponse, AttesterIncentiveErrors<StateAccessorError<S::Gas>>> {
        // First get the bonded attester
        if let Some(bond) = self.bonded_attesters.get(context.sender(), state)? {
            let finalized_height = self
                .light_client_finalized_height
                .get(state)?
                .expect("Must be set at genesis");

            // Remove the attester from the bonding set
            self.bonded_attesters.remove(context.sender(), state)?;

            // Then add the bonded attester to the unbonding set, with the current finalized height
            self.unbonding_attesters.set(
                context.sender(),
                &UnbondingInfo {
                    unbonding_initiated_height: finalized_height,
                    amount: bond,
                },
                state,
            )?;
        }

        Ok(CallResponse::default())
    }

    pub(crate) fn exit_attester(
        &self,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<CallResponse, AttesterIncentiveErrors<StateAccessorError<S::Gas>>> {
        // We have to ensure that the attester is unbonding, and that the unbonding transaction
        // occurred at least `finality_period` blocks ago to let the attester unbond
        if let Some(unbonding_info) = self.unbonding_attesters.get(context.sender(), state)? {
            // These two constants should always be set beforehand, hence we can panic if they're not set
            let curr_height = self
                .light_client_finalized_height
                .get(state)?
                .expect("Should be defined at genesis");

            let finality_period = self
                .rollup_finality_period
                .get(state)?
                .expect("Should be defined at genesis");

            if unbonding_info
                .unbonding_initiated_height
                .saturating_add(finality_period)
                > curr_height
            {
                return Err(AttesterIncentiveErrors::UnbondingNotFinalized);
            }

            // Get the user's old balance.
            // Transfer the bond amount from the sender to the module's id.
            // On failure, no state is changed
            self.transfer_tokens_to_sender(context, unbonding_info.amount, state)
                .map_err(|_err| AttesterIncentiveErrors::RewardTransferFailure)?;

            // Update our internal tracking of the total bonded amount for the sender.
            self.bonded_attesters.remove(context.sender(), state)?;
            self.unbonding_attesters.remove(context.sender(), state)?;

            self.emit_event(
                state,
                Event::<S>::ExitedAttester {
                    amount_withdrawn: unbonding_info.amount,
                },
            );
        } else {
            return Err(AttesterIncentiveErrors::AttesterIsNotUnbonding);
        }
        Ok(CallResponse::default())
    }
}
