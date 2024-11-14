//! Defines the `BankAccount` struct and its methods

use indexmap::IndexSet;
use sov_bank::{Coins, TokenId};
use sov_modules_api::prelude::arbitrary;
use sov_modules_api::{CryptoSpec, Spec};

use super::Tag;
use crate::interface::{PickRandom, TagAction, Taggable};
use crate::state::{AccountState, AccountStateView, ApplyTo};

/// The view of an account used by the bank message generator
#[derive(Clone, Debug)]
pub struct BankAccount<S: Spec> {
    /// The account's private key
    pub private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
    /// All tokens of which the account has a non-zero balance
    balances: Vec<Coins>,
    /// The set of tokens that the account is allowed to mint
    can_mint: IndexSet<TokenId>,

    tag_changes: Vec<TagAction<Tag>>,
}

impl<S: Spec> Taggable for BankAccount<S> {
    type Tag = Tag;

    fn take_tags(&mut self) -> impl IntoIterator<Item = TagAction<Self::Tag>> {
        std::mem::take(&mut self.tag_changes)
    }

    fn add_tag(&mut self, tag: Self::Tag) {
        self.tag_changes.push(TagAction::Add(tag));
    }

    fn remove_tag(&mut self, tag: Self::Tag) {
        self.tag_changes.push(TagAction::Remove(tag));
    }
}

impl<S: Spec> BankAccount<S> {
    /// Set this account as being able to mint the given token
    pub fn add_can_mint(&mut self, token_id: TokenId) {
        self.can_mint.insert(token_id);
        self.add_tag(Tag::CanMint);
        self.add_tag(Tag::CanMintById(token_id));
    }

    /// Set this account as being unable to mint the given token
    pub fn remove_can_mint(&mut self, token_id: TokenId) {
        self.can_mint.swap_remove(&token_id);
        self.remove_tag(Tag::CanMintById(token_id));
        if self.can_mint.is_empty() {
            self.remove_tag(Tag::CanMint);
        }
    }

    /// Borrows the set of tokens that this account can mint
    pub fn can_mint(&self) -> &IndexSet<TokenId> {
        &self.can_mint
    }

    /// Increments the balance in place. Returns a copy of the new balance.
    pub fn increment_balance(&mut self, coins: Coins) -> u64 {
        let Coins { amount, token_id } = coins;
        // If we're not actually changing the balance, don't add the token.
        // This keeps our balances array from getting cluttered with zero balances
        if amount == 0 {
            self.find(token_id)
                .map(|coins| coins.amount)
                .unwrap_or_default()
        } else {
            self.add_tag(Tag::HasBalance);
            let balance = self.find_or_insert(token_id);
            balance.amount += amount;
            balance.amount
        }
    }

    /// Find the balance of the given token
    pub fn balance_of(&self, token_id: TokenId) -> u64 {
        self.find(token_id).map(|coins| coins.amount).unwrap_or(0)
    }

    /// The maximum amount of the given token that can be received without overflowing
    pub fn receivable_balance(&self, token_id: TokenId) -> u64 {
        self.find(token_id)
            .map(|coins| u64::MAX - coins.amount)
            .unwrap_or(u64::MAX)
    }

    /// Decrements the old balance in place, removing the entry if the balance is drained. Returns a copy of the new balance
    pub fn decrement_balance(&mut self, coins: Coins) -> u64 {
        let Coins { amount, token_id } = coins;
        let existing = self.find_or_insert(token_id);
        assert!(
            existing.amount >= amount,
            "Tried to subtract more than the existing balance. This is a bug in the generator."
        );
        existing.amount -= amount;
        let remaining = existing.amount;
        // If there's no more balance of this coin, remove it from the balances list
        if remaining == 0 {
            self.remove_token(coins.token_id);
        }

        remaining
    }

    /// Removes a token from the balances list by ID
    pub fn remove_token(&mut self, token_id: TokenId) {
        let index = self
            .balances
            .iter()
            .position(|balance| balance.token_id == token_id)
            .unwrap();
        self.balances.remove(index);
        if self.balances.is_empty() {
            self.remove_tag(Tag::HasBalance);
        }
    }

    /// Picks a balance at random from the balances list, if possible.
    pub fn pick_random_balance(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<Option<&Coins>> {
        if self.balances.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.balances.random_entry(u)?))
    }

    /// find the balance of token_id
    fn find(&self, token_id: TokenId) -> Option<&Coins> {
        self.balances
            .iter()
            .find(|balance| balance.token_id == token_id)
    }

    /// Return a reference to the balances entry for the given token, creating one
    /// with zero balance if necessary. Callers should be careful to delete the entry
    /// if they don't update the balance.
    fn find_or_insert(&mut self, token_id: TokenId) -> &mut Coins {
        // We use a somewhat convoluted method to get the correct balance by index here because the borrow checker
        // couldn't infer the correct lifetimes if we used iter_mut or the `find` method.

        let Some((idx, _)) = self
            .balances
            .iter()
            .enumerate()
            .find(|balance| balance.1.token_id == token_id)
        else {
            self.balances.push(Coins {
                amount: 0,
                token_id,
            });
            return self
                .balances
                .last_mut()
                .expect("Balances cannot be empty because we just appended an entry.");
        };

        return self
            .balances
            .get_mut(idx)
            .expect("We just checked that the entry was present.");
    }
}

impl<S: Spec, T> From<&AccountState<S, T>> for BankAccount<S> {
    fn from(value: &AccountState<S, T>) -> BankAccount<S> {
        BankAccount {
            private_key: value.private_key.clone(),
            balances: value.balances.clone(),
            can_mint: value.can_mint.clone(),
            tag_changes: Default::default(),
        }
    }
}

impl<S: Spec, Tag, Data> From<&AccountStateView<S, Tag, Data>> for BankAccount<S> {
    fn from(value: &AccountStateView<S, Tag, Data>) -> BankAccount<S> {
        BankAccount {
            private_key: value
                .private_key
                .as_ref()
                .expect("Cannot construct bank account from empty account view")
                .clone(),
            balances: value
                .balances
                .as_ref()
                .expect("Cannot construct bank account from empty account view")
                .clone(),
            can_mint: value
                .can_mint
                .as_ref()
                .expect("Cannot construct bank account from empty account view")
                .clone(),
            tag_changes: Default::default(),
        }
    }
}

impl<S: Spec, Tag: From<super::Tag>, Data> ApplyTo<AccountStateView<S, Tag, Data>>
    for BankAccount<S>
{
    fn apply_to(self, account: &mut AccountStateView<S, Tag, Data>) {
        account.balances = Some(self.balances);
        account.can_mint = Some(self.can_mint);
        account
            .tag_changes
            .extend(self.tag_changes.into_iter().map(|t| t.map(Into::into)));
    }
}
