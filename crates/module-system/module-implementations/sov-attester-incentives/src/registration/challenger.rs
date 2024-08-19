use core::result::Result::Ok;

use sov_modules_api::registration_lib::StakeRegistration;
use sov_modules_api::{CallResponse, Context, EventEmitter, StateAccessor, TxState};
use sov_state::EventContainer;

use super::{AttesterRegistryError, Staker};
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
        let challenger = Staker::new_challenger(self);
        challenger.register_staker(user_address, user_address, bond_amount, state)?;

        self.emit_event(
            state,
            Event::<S>::RegisteredChallenger {
                amount: bond_amount,
            },
        );

        Ok(CallResponse::default())
    }

    /// Try to unbond the requested amount of coins with context.sender() as the beneficiary.
    pub(crate) fn exit_challenger(
        &self,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> anyhow::Result<CallResponse> {
        let challenger = Staker::new_challenger(self);
        let amount_withdrawn = challenger.exit_staker(context.sender(), state)?;

        self.emit_event(state, Event::<S>::ExitedChallenger { amount_withdrawn });

        Ok(CallResponse::default())
    }
}
