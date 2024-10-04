use sov_bank::{CallMessage, Coins, TokenId};
use sov_modules_api::prelude::arbitrary::{self, Arbitrary};
use sov_modules_api::Spec;

use super::{
    BankAccount, BankChangeLogEntry, BankMessageGenerator, InternalMessageGenError,
    InternalMessageGenResult,
};
use crate::interface::{GeneratedMessage, GeneratorState, MessageValidity, RandomUniform};

impl<S: Spec> BankMessageGenerator<S> {
    /// Generate a bank transfer message
    pub(super) fn generate_transfer(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
        _rollup_state_accessor: &(),
        generator_state: &mut impl GeneratorState<S, AccountView = BankAccount<S>>,
        validity: MessageValidity,
    ) -> InternalMessageGenResult<GeneratedMessage<S, CallMessage<S>, BankChangeLogEntry<S>>> {
        let (from_addr, mut from_account) =
            Self::get_random_account_with_balance(generator_state, u)?;
        if validity.is_invalid() {
            // A transfer can be invalid by...
            // ... transferring a token ID that doesn't exist
            // ... transferring from an account that doesn't exist
            // ... transferring from an account that doesn't hold the token
            // ... transferring more tokens than the account has.
            // In a future PR, we will generate each kind of invalid tx - but for brevity, we implement case 1 here.
            let to_address = S::Address::arbitrary(u)?;
            let message = CallMessage::Transfer {
                to: to_address,
                coins: Coins {
                    amount: 1,
                    // The probability that a random TokenID exists is the same as that of a hash collision
                    token_id: TokenId::arbitrary(u)?,
                },
            };
            return Ok(GeneratedMessage::new(message, from_addr, Vec::new()));
        }

        // Pick a random amount of a random token, and a random address to send it to
        let from_balance = from_account
            .pick_random_balance(u)?
            .expect("We picked a non-empty account");
        let token_id = from_balance.token_id;
        // A valid transfer has to come from an existing address
        // but it can go to a new one or an existing one
        let (to_addr, mut to_account) =
            generator_state.get_or_generate(self.address_creation_rate, u)?;
        let balance_to_send = u64::less_than(&(from_balance.amount + 1), u)?;

        // Construct the call message
        let coins_to_send = Coins {
            amount: balance_to_send,
            token_id,
        };
        let msg = CallMessage::Transfer {
            to: to_addr.clone(),
            coins: coins_to_send.clone(),
        };

        // If this is a self-transfer, then it should be a no-op. No balances need to change,
        // and there are no state changes to cache.
        if from_addr == to_addr {
            return Ok(GeneratedMessage::new(msg, from_addr, vec![]));
        }

        // Otherwise, account for the balance changes
        let receiver_balance = to_account.increment_balance(coins_to_send.clone());
        from_balance.amount -= balance_to_send;
        let remaining_from_balance = from_balance.amount;
        if remaining_from_balance == 0 {
            from_account.remove_token(token_id);
        }

        // Save back the modified state and compute the changelog
        generator_state.update_account(from_addr.clone(), from_account);
        generator_state.update_account(to_addr.clone(), to_account);
        let changelog_entries = vec![
            BankChangeLogEntry::balance_changed(
                from_addr.clone(),
                token_id,
                remaining_from_balance,
            ),
            BankChangeLogEntry::balance_changed(to_addr, token_id, receiver_balance),
        ];

        // Finally, return the generated message
        Ok(GeneratedMessage::new(msg, from_addr, changelog_entries))
    }

    fn get_random_account_with_balance(
        generator_state: &mut impl GeneratorState<S, AccountView = BankAccount<S>>,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> InternalMessageGenResult<(S::Address, BankAccount<S>)> {
        for _ in 0..10_000 {
            let (addr, account) = generator_state.get_random_existing_account(u)?;
            if account.balances.is_empty() {
                continue;
            }
            return Ok((addr, account));
        }
        Err(InternalMessageGenError::NoAccountWithBalance)
    }
}
