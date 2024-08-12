use core::result::Result::Ok;

use sov_modules_api::registration_lib::RegistrationError;
use sov_modules_api::{CallResponse, Context, EventEmitter, StateAccessor, TxState};
use sov_state::EventContainer;

use super::AttesterRegistryError;
use crate::{AttesterIncentives, Event};

impl<S, Da> AttesterIncentives<S, Da>
where
    S: sov_modules_api::Spec,
    Da: sov_modules_api::DaSpec,
{
    pub(crate) fn register_challenger<ST: StateAccessor + EventContainer>(
        &self,
        bond_amount: u64,
        user_address: &S::Address,
        state: &mut ST,
    ) -> Result<CallResponse, AttesterRegistryError<S, ST>> {
        if self.bonded_challengers.get(user_address, state)?.is_some() {
            return Err(RegistrationError::AlreadyRegistered(user_address.clone()));
        }

        let minimum_bond = self
            .minimum_challenger_bond
            .get(state)?
            .ok_or(RegistrationError::NoMinimumBondSet(user_address.clone()))?;

        if bond_amount < minimum_bond {
            return Err(RegistrationError::InsufficientStakeAmount {
                address: user_address.clone(),
                bond_amount,
                minimum_bond_amount: minimum_bond,
            });
        }

        let balances = &self.bonded_challengers;
        self.register_user_helper::<ST>(bond_amount, user_address, balances, state)?;

        let event = Event::<S>::RegisteredChallenger {
            amount: bond_amount,
        };

        self.emit_event(state, event);
        Ok(CallResponse::default())
    }

    /// Try to unbond the requested amount of coins with context.sender() as the beneficiary.
    pub(crate) fn exit_challenger(
        &self,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<CallResponse> {
        // Get the user's old balance.
        if let Some(old_balance) = self.bonded_challengers.get(context.sender(), state)? {
            // Transfer the bond amount from the sender to the module's id.
            // On failure, no state is changed
            self.transfer_tokens_to_sender(context, old_balance, state)?;

            // Emit the unbonding event
            self.emit_event(
                state,
                Event::<S>::ExitedChallenger {
                    amount_withdrawn: old_balance,
                },
            );
        }

        Ok(CallResponse::default())
    }
}
