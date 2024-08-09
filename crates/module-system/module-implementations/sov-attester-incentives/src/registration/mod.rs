pub(crate) mod attester;
pub(crate) mod challenger;

use core::result::Result::Ok;

use anyhow::Context as AnyhowContext;
use sov_bank::{Amount, Coins, IntoPayable, GAS_TOKEN_ID};
use sov_modules_api::{StateAccessor, StateReader};
use sov_state::{EventContainer, User};

use crate::{AttesterIncentiveErrors, AttesterIncentives};

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
    ) -> Result<Amount, AttesterIncentiveErrors<<ST as StateReader<User>>::Error>> {
        // Transfer the bond amount from the sender to the module's id.
        // On failure, no state is changed
        let coins = Coins {
            token_id: GAS_TOKEN_ID,
            amount: bond_amount,
        };

        self.bank
            .transfer_from(user_address, self.id.to_payable(), coins, state)
            .map_err(|_err| AttesterIncentiveErrors::BondTransferFailure)?;

        // Update our record of the total bonded amount for the sender.
        // This update is infallible, so no value can be destroyed.
        let old_balance = balances.get(user_address, state)?.unwrap_or_default();

        let total_balance = old_balance
            .checked_add(bond_amount)
            .with_context(|| {
                anyhow::anyhow!("The total balance overflows with the given operation")
            })
            .map_err(|_| AttesterIncentiveErrors::BondTransferFailure)?;

        balances.set(user_address, &total_balance, state)?;

        Ok(total_balance)
    }
}
