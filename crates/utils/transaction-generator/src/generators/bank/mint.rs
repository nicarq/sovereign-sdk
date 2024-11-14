use sov_bank::{Amount, CallMessage, Coins, TokenId};
use sov_modules_api::prelude::arbitrary::{self, Arbitrary};
use sov_modules_api::Spec;

use super::{
    BankAccount, BankChangeLogEntry, BankMessageGenerator, InternalMessageGenError,
    InternalMessageGenResult, Tag,
};
use crate::interface::{GeneratedMessage, GeneratorState, MessageValidity, PickRandom};
use crate::state::TokenInfo;

impl<S: Spec> BankMessageGenerator<S> {
    pub(super) fn generate_mint(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
        generator_state: &mut impl GeneratorState<S, AccountView = BankAccount<S>, Tag = Tag>,
        validity: MessageValidity,
    ) -> InternalMessageGenResult<GeneratedMessage<S, CallMessage<S>, BankChangeLogEntry<S>>> {
        // A mint can be invalid because...
        //  - The token doesn't exist
        //  - The caller isn't authorized for the token
        //  - The mint amount would cause the balance to overflow
        if validity.is_invalid() {
            // Track whether the message is currently valid
            let mut is_valid = false;
            let (_, acct, mut token_id) = if bool::arbitrary(u)? {
                // Half the time, we try to get an invalid account
                Self::get_random_non_minting_account(generator_state, u)?
            } else {
                // The other half, we try to get a valid account but fall back to invalid if that fails.
                if let Some((addr, acct)) =
                    generator_state.get_random_existing_account_with_tag(Tag::CanMint, u)?
                {
                    let token = *acct.can_mint().random_entry(u)?;
                    is_valid = true;
                    (addr, acct, token)
                } else {
                    Self::get_random_non_minting_account(generator_state, u)?
                }
            };
            let key = acct.private_key.clone();

            // All amounts are valid unless they overflow the account or the token supply
            let token_info = generator_state.get_token(&token_id).expect("Token exists");
            let amount = {
                // Flip a coin to see if we should try to overflow.
                if bool::arbitrary(u)? {
                    // Generate an amount large enough to overflow if possible
                    let max_amount_without_overflow =
                        std::cmp::min(acct.receivable_balance(token_id), token_info.total_supply);
                    if max_amount_without_overflow.saturating_add(2) < Amount::MAX {
                        is_valid = false;
                        u.int_in_range((max_amount_without_overflow + 1)..=Amount::MAX)?
                    } else {
                        Amount::arbitrary(u)?
                    }
                } else {
                    Amount::arbitrary(u)?
                }
            };

            // Use a non-existent token id if we need to to make the message invalid,
            // plus do it 1/2 the time even if the message was already invalid
            if is_valid || bool::arbitrary(u)? {
                // An arbitrary token ID will never exist
                token_id = TokenId::arbitrary(u)?;
            };
            let message = CallMessage::Mint {
                coins: Coins { token_id, amount },
                mint_to_address: S::Address::arbitrary(u)?,
            };
            return Ok(GeneratedMessage::new(message, key, Vec::new()));
        }

        for _ in 0..1_000 {
            let (_, acct) = generator_state
                .get_random_existing_account_with_tag(Tag::CanMint, u)?
                .ok_or(InternalMessageGenError::NoMintingAccounts)?;
            let token_id = *acct.can_mint().random_entry(u)?;
            let token_info = generator_state.get_token(&token_id).expect("Token exists");
            if token_info.total_supply == Amount::MAX {
                continue;
            }

            let key = acct.private_key.clone();
            let (recipient_addr, mut recipient_acct) =
                generator_state.get_or_generate(self.address_creation_rate, u)?;

            let amount_to_mint = u.int_in_range(0..=Amount::MAX - token_info.total_supply)?;
            let recipient_balance = recipient_acct.increment_balance(Coins {
                token_id,
                amount: amount_to_mint,
            });

            let message = CallMessage::Mint {
                coins: Coins {
                    token_id,
                    amount: amount_to_mint,
                },
                mint_to_address: recipient_addr.clone(),
            };
            let mint_change = Self::update_state_with_mint(
                generator_state,
                token_id,
                token_info,
                amount_to_mint,
                u,
            )?;
            let changes = vec![
                BankChangeLogEntry::balance_changed(
                    recipient_addr.clone(),
                    token_id,
                    recipient_balance,
                ),
                mint_change,
            ];

            generator_state.update_account(recipient_addr, recipient_acct);

            return Ok(GeneratedMessage::new(message, key, changes));
        }
        Err(InternalMessageGenError::NoMintingAccounts)
    }

    /// Gets a random account and a token ID for which the minter is *not* authorized
    fn get_random_non_minting_account(
        generator_state: &mut impl GeneratorState<S, AccountView = BankAccount<S>, Tag: From<Tag>>,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> InternalMessageGenResult<(S::Address, BankAccount<S>, TokenId)> {
        // We'll try 1000 times to get an account that can't mint. This is arbitrary
        for _ in 0..1_000 {
            let (address, acct) = generator_state.get_random_existing_account(u)?;
            // For each account, pick up to 10 tokens at random until we find one that the account
            // isn't authorized to mint.
            for _ in 0..10 {
                let (token_id, _info) = generator_state.get_random_token(u)?;
                if !acct.can_mint().contains(&token_id) {
                    return Ok((address, acct, token_id));
                }
            }
        }
        // If we haven't succeeded after 1000 attempts, throw an error
        tracing::warn!("Unable to find non-minting account");
        Err(InternalMessageGenError::NonMintingAccountNotFound)
    }

    pub(super) fn update_state_with_mint(
        state: &mut impl GeneratorState<S, AccountView = BankAccount<S>, Tag = Tag>,
        token_id: TokenId,
        mut old_token_info: TokenInfo,
        amount_minted: Amount,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<BankChangeLogEntry<S>> {
        let new_supply = old_token_info
            .total_supply
            .checked_add(amount_minted)
            .expect("Token amount should have been checked for overflow");
        // If the supply is maxed, update our index to account for that, and remove the tag from affected accounts
        if new_supply == Amount::MAX {
            while let Some((addr, mut acct)) =
                state.get_random_existing_account_with_tag(Tag::CanMintById(token_id), u)?
            {
                acct.remove_can_mint(token_id);
                state.update_account(addr, acct);
            }
        }

        old_token_info.total_supply = new_supply;
        state.update_token(token_id, old_token_info);

        Ok(BankChangeLogEntry::supply_changed(token_id, new_supply))
    }
}
