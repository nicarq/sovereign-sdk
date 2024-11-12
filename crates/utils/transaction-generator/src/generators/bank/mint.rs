use sov_bank::{Amount, CallMessage, Coins, TokenId};
use sov_modules_api::prelude::arbitrary::{self, Arbitrary};
use sov_modules_api::Spec;

use super::{
    BankAccount, BankChangeLogEntry, BankMessageGenerator, InternalMessageGenError,
    InternalMessageGenResult, Tag,
};
use crate::interface::{GeneratedMessage, GeneratorState, MessageValidity, PickRandom};

impl<S: Spec> BankMessageGenerator<S> {
    pub(super) fn generate_mint(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
        _rollup_state_accessor: &(),
        generator_state: &mut impl GeneratorState<S, AccountView = BankAccount<S>, Tag: From<Tag>>,
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
                    let token = *acct.can_mint.random_entry(u)?;
                    is_valid = true;
                    (addr, acct, token)
                } else {
                    Self::get_random_non_minting_account(generator_state, u)?
                }
            };
            let key = acct.private_key.clone();

            // All amounts are valid unless they overflow
            let amount = {
                // Flip a coin to see if we should try to overflow.
                if bool::arbitrary(u)? {
                    // Generate an amount large enough to overflow if possible
                    let max_amount_without_overflow = acct.receivable_balance(token_id);
                    if max_amount_without_overflow.saturating_add(2) < u64::MAX {
                        is_valid = false;
                        u.int_in_range((max_amount_without_overflow + 1)..=u64::MAX)?
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

        let (_, acct) = generator_state
            .get_random_existing_account_with_tag(Tag::CanMint, u)?
            .ok_or(InternalMessageGenError::NoMintingAccounts)?;
        let key = acct.private_key.clone();
        let token_id = *acct.can_mint.random_entry(u)?;

        for _ in 0..1_000 {
            let (recipient_addr, mut recipient_acct) =
                generator_state.get_or_generate(self.address_creation_rate, u)?;
            let balance = recipient_acct.find_or_insert(token_id);
            if balance.amount == u64::MAX {
                continue;
            }

            let amount_to_mint = u.int_in_range(0..=u64::MAX - balance.amount)?;
            balance.amount += amount_to_mint;

            let message = CallMessage::Mint {
                coins: Coins {
                    token_id,
                    amount: amount_to_mint,
                },
                mint_to_address: recipient_addr.clone(),
            };
            let changes = vec![BankChangeLogEntry::balance_changed(
                recipient_addr.clone(),
                token_id,
                balance.amount,
            )];

            generator_state.update_account(recipient_addr, recipient_acct, vec![]);

            return Ok(GeneratedMessage::new(message, key, changes));
        }
        Err(InternalMessageGenError::NoMintingAccounts)
    }

    /// Gets a random account and a token ID for which the minter is *not* authorized
    fn get_random_non_minting_account(
        generator_state: &mut impl GeneratorState<S, AccountView = BankAccount<S>, Tag: From<Tag>>,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> InternalMessageGenResult<(S::Address, BankAccount<S>, TokenId)> {
        for _ in 0..1_000 {
            let (address, acct) = generator_state.get_random_existing_account(u)?;
            for balance in acct.balances.iter() {
                if !acct.can_mint.contains(&balance.token_id) {
                    return Ok((address, acct.clone(), balance.token_id));
                }
            }
        }
        tracing::warn!("Unable to find non-minting account");
        Err(InternalMessageGenError::NoNonMintingAccounts)
    }
}
