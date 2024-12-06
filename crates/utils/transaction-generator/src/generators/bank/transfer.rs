use sov_bank::{CallMessage, Coins, TokenId};
use sov_modules_api::prelude::arbitrary::{self, Arbitrary};
use sov_modules_api::Spec;

use super::{
    BankAccount, BankChangeLogEntry, BankMessageGenerator, InternalMessageGenError,
    InternalMessageGenResult, Tag,
};
use crate::interface::{GeneratedMessage, GeneratorState};
use crate::repeatedly;

#[derive(Arbitrary, Clone, Debug)]
enum InvalidTransferReasons {
    InvalidTokenID,
    AccountDoesNotExist,
    // TODO
    // AccountDoesNotHoldToken,
    // NotEnoughFunds,
}

impl<S: Spec> BankMessageGenerator<S> {
    fn generate_invalid_transfer_helper(
        &self,
        error_reason: InvalidTransferReasons,
        u: &mut arbitrary::Unstructured<'_>,
        generator_state: &mut impl GeneratorState<S, AccountView = BankAccount<S>, Tag: From<Tag>>,
    ) -> arbitrary::Result<GeneratedMessage<S, CallMessage<S>, BankChangeLogEntry<S>>> {
        let from_key = {
            if let InvalidTransferReasons::AccountDoesNotExist = error_reason {
                Arbitrary::arbitrary(u)?
            } else {
                let Ok((_, from_account)) =
                    Self::get_random_account_with_balance(generator_state, u)
                else {
                    // If we can't find any account with balance, we try again with a different error reason
                    return self.generate_invalid_transfer(u, generator_state);
                };

                from_account.private_key
            }
        };

        let to_address = S::Address::arbitrary(u)?;
        let message = CallMessage::Transfer {
            to: to_address,
            coins: Coins {
                amount: 1,
                // The probability that a random TokenID exists is the same as that of a hash collision
                token_id: TokenId::arbitrary(u)?,
            },
        };

        Ok(GeneratedMessage::new(message, from_key, Vec::new()))
    }

    /// A transfer can be invalid by:
    /// - transferring a token ID that doesn't exist
    /// - transferring from an account that doesn't exist
    /// - transferring from an account that doesn't hold the token
    /// - transferring more tokens than the account has.
    /// In a future PR, we will generate each kind of invalid tx - but for brevity, we implement case 1 here.
    pub(crate) fn generate_invalid_transfer(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
        generator_state: &mut impl GeneratorState<S, AccountView = BankAccount<S>, Tag: From<Tag>>,
    ) -> arbitrary::Result<GeneratedMessage<S, CallMessage<S>, BankChangeLogEntry<S>>> {
        let invalid_reason = InvalidTransferReasons::arbitrary(u)?;
        self.generate_invalid_transfer_helper(invalid_reason, u, generator_state)
    }

    /// Generate a bank transfer message
    // we'll be able to use the trait methods directly for testing.
    #[allow(private_interfaces)]
    pub(crate) fn generate_valid_transfer(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
        generator_state: &mut impl GeneratorState<S, AccountView = BankAccount<S>, Tag: From<Tag>>,
    ) -> InternalMessageGenResult<GeneratedMessage<S, CallMessage<S>, BankChangeLogEntry<S>>> {
        let (from_addr, mut from_account) =
            Self::get_random_account_with_balance(generator_state, u)?;
        let from_key = from_account.private_key.clone();

        // Pick a random amount of a random token, and a random address to send it to
        let from_balance = from_account.pick_random_balance(u)?.unwrap_or_else(|| {
            panic!(
                "We picked a non-empty account but {} had no tokens",
                from_addr
            )
        });

        let token_id = from_balance.token_id;
        // A valid transfer has to come from an existing address
        // but it can go to a new one or an existing one
        let balance_to_send = u.int_in_range(1..=from_balance.amount)?;

        // Find a recipient who can receive that much balance
        repeatedly! {
            let (to_addr, to_account) = generator_state.get_or_generate(self.address_creation_rate, u)?;
            until: balance_to_send <= to_account.receivable_balance(token_id),
            on_failure: return Err(InternalMessageGenError::NoAccountsCanReceive(Coins {
                token_id,
                amount: balance_to_send,
            })
        )};
        let mut to_account = to_account;

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
            return Ok(GeneratedMessage::new(msg, from_key, vec![]));
        }

        // Otherwise, account for the balance changes
        let receiver_balance = to_account.increment_balance(coins_to_send.clone());
        let remaining_from_balance = from_account.decrement_balance(coins_to_send.clone());
        // Save back the modified state and compute the changelog
        generator_state.update_account(&to_addr, to_account);
        generator_state.update_account(&from_addr, from_account);
        let changelog_entries = vec![
            BankChangeLogEntry::balance_changed(
                from_addr.clone(),
                token_id,
                remaining_from_balance,
            ),
            BankChangeLogEntry::balance_changed(to_addr, token_id, receiver_balance),
        ];

        // Finally, return the generated message
        Ok(GeneratedMessage::new(msg, from_key, changelog_entries))
    }

    fn get_random_account_with_balance(
        generator_state: &mut impl GeneratorState<S, AccountView = BankAccount<S>, Tag: From<Tag>>,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> InternalMessageGenResult<(S::Address, BankAccount<S>)> {
        generator_state
            .get_random_existing_account_with_tag(Tag::HasBalance, u)?
            .ok_or(InternalMessageGenError::NoAccountWithBalance)
    }
}
