use core::result::Result::Ok;

use anyhow::Context as AnyhowContext;
use sov_bank::{Coins, IntoPayable, GAS_TOKEN_ID};
use sov_modules_api::{CallResponse, Context, EventEmitter, StateAccessor, TxState};
use sov_state::EventContainer;

use crate::{AttesterIncentiveErrors, AttesterIncentives, Event, Role, UnbondingInfo};

impl<S, Da> AttesterIncentives<S, Da>
where
    S: sov_modules_api::Spec,
    Da: sov_modules_api::DaSpec,
{
    /// A helper function for the `bond_challenger/attester` call. Also used to bond challengers/attesters
    /// during genesis when no context is available.
    pub(super) fn bond_user_helper(
        &self,
        bond_amount: u64,
        user_address: &S::Address,
        role: Role,
        state: &mut (impl StateAccessor + EventContainer),
    ) -> Result<CallResponse, AttesterIncentiveErrors> {
        // If the user is an attester, we have to check that he's not trying to unbond
        if role == Role::Attester
            && self
                .unbonding_attesters
                .get(user_address, state)
                .map_err(|e| AttesterIncentiveErrors::StateAccessError(e.to_string()))?
                .is_some()
        {
            return Err(AttesterIncentiveErrors::AttesterIsUnbonding);
        }

        // Transfer the bond amount from the sender to the module's id.
        // On failure, no state is changed
        let coins = Coins {
            token_id: GAS_TOKEN_ID,
            amount: bond_amount,
        };

        self.bank
            .transfer_from(user_address, self.id.to_payable(), coins, state)
            .map_err(|_err| AttesterIncentiveErrors::BondTransferFailure)?;

        let balances = match role {
            Role::Attester => &self.bonded_attesters,
            Role::Challenger => &self.bonded_challengers,
        };

        // Update our record of the total bonded amount for the sender.
        // This update is infallible, so no value can be destroyed.
        let old_balance = balances
            .get(user_address, state)
            .map_err(|e| AttesterIncentiveErrors::StateAccessError(e.to_string()))?
            .unwrap_or_default();
        let total_balance = old_balance
            .checked_add(bond_amount)
            .with_context(|| {
                anyhow::anyhow!("The total balance overflows with the given operation")
            })
            .map_err(|_| AttesterIncentiveErrors::BondTransferFailure)?;
        balances
            .set(user_address, &total_balance, state)
            .map_err(|e| AttesterIncentiveErrors::StateAccessError(e.to_string()))?;

        // Emit the bonding event
        match role {
            Role::Attester => self.emit_event(
                state,
                Event::<S>::BondedAttester {
                    new_deposit: bond_amount,
                    total_bond: total_balance,
                },
            ),
            Role::Challenger => self.emit_event(
                state,
                Event::<S>::BondedChallenger {
                    new_deposit: bond_amount,
                    total_bond: total_balance,
                },
            ),
        }

        Ok(CallResponse::default())
    }

    /// Try to unbond the requested amount of coins with context.sender() as the beneficiary.
    pub(crate) fn unbond_challenger(
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
                Event::<S>::UnbondedChallenger {
                    amount_withdrawn: old_balance,
                },
            );
        }

        Ok(CallResponse::default())
    }

    /// The attester starts the first phase of the two-phase unbonding.
    /// We put the current max finalized height with the attester address
    /// in the set of unbonding attesters if the attester
    /// is already present in the unbonding set
    pub(crate) fn begin_unbond_attester(
        &self,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<CallResponse, AttesterIncentiveErrors> {
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

    pub(crate) fn end_unbond_attester(
        &self,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<CallResponse, AttesterIncentiveErrors> {
        // We have to ensure that the attester is unbonding, and that the unbonding transaction
        // occurred at least `finality_period` blocks ago to let the attester unbond
        if let Some(unbonding_info) = self
            .unbonding_attesters
            .get(context.sender(), state)
            .map_err(|e| AttesterIncentiveErrors::StateAccessError(e.to_string()))?
        {
            // These two constants should always be set beforehand, hence we can panic if they're not set
            let curr_height = self
                .light_client_finalized_height
                .get(state)
                .map_err(|e| AttesterIncentiveErrors::StateAccessError(e.to_string()))?
                .expect("Should be defined at genesis");
            let finality_period = self
                .rollup_finality_period
                .get(state)
                .map_err(|e| AttesterIncentiveErrors::StateAccessError(e.to_string()))?
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
            self.transfer_tokens_to_sender(context, unbonding_info.amount, state)?;

            // Update our internal tracking of the total bonded amount for the sender.
            self.bonded_attesters.remove(context.sender(), state)?;
            self.unbonding_attesters.remove(context.sender(), state)?;

            self.emit_event(
                state,
                Event::<S>::UnbondedAttester {
                    amount_withdrawn: unbonding_info.amount,
                },
            );
        } else {
            return Err(AttesterIncentiveErrors::AttesterIsNotUnbonding);
        }
        Ok(CallResponse::default())
    }
}
