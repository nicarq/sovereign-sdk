use std::collections::HashMap;
use std::marker::PhantomData;

use arbitrary::Arbitrary;
use indexmap::{IndexMap, IndexSet};
use sov_modules_api::prelude::arbitrary;

use super::interface::{CallMessageGenerator, GeneratorState, PickRandom, TagAction};
pub trait TransactionGenerator {
    /// Generate a transaction
    fn generate_transaction(&mut self, u: arbitrary::Unstructured<'_>);
}

use sov_bank::{Coins, TokenId};
use sov_modules_api::{CryptoSpec, PrivateKey, Spec};

use super::bank::message_generator::BankAccount;

#[derive(Clone, Debug)]
pub struct AccountState<S: Spec, T = ()> {
    pub balances: Vec<Coins>,
    pub can_mint: IndexSet<TokenId>,
    pub sequencing_bond: Option<u64>,
    pub private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
    pub additional_info: T,
}

impl<S: Spec, T> From<&AccountState<S, T>> for BankAccount<S> {
    fn from(value: &AccountState<S, T>) -> BankAccount<S> {
        BankAccount {
            private_key: value.private_key.clone(),
            balances: value.balances.clone(),
            can_mint: value.can_mint.clone(),
        }
    }
}

impl<S: Spec> ApplyToAccount<S, ()> for BankAccount<S> {
    fn apply_to(self, state: &mut AccountState<S, ()>) {
        assert_eq!(
            state.private_key.pub_key(),
            self.private_key.pub_key(),
            "Applied to wrong account!"
        );
        state.balances = self.balances;
        state.can_mint = self.can_mint;
    }
}

/// Allows a state view to update the global state.
pub trait ApplyToAccount<S: Spec, T> {
    /// Applies any changes to a view onto the global account state
    fn apply_to(self, state: &mut AccountState<S, T>);
}

pub struct State<S: Spec, M: CallMessageGenerator<S>, T = ()> {
    accounts: IndexMap<S::Address, AccountState<S, T>>,
    tags: HashMap<M::Tag, IndexSet<S::Address>>,
    phantom_module: PhantomData<M>,
}

impl<S: Spec, M: CallMessageGenerator<S>, T> Default for State<S, M, T> {
    fn default() -> Self {
        Self {
            accounts: Default::default(),
            phantom_module: Default::default(),
            tags: Default::default(),
        }
    }
}

impl<S: Spec, M: CallMessageGenerator<S>, T> State<S, M, T> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<S: Spec, M: CallMessageGenerator<S>, T: Default + 'static> GeneratorState<S> for State<S, M, T>
where
    for<'a> M::AccountView: From<&'a AccountState<S, T>>,
    M::AccountView: ApplyToAccount<S, T>,
{
    type AccountView = M::AccountView;

    type Tag = M::Tag;

    fn get_account(&self, address: S::Address) -> Option<Self::AccountView> {
        self.accounts.get(&address).map(Into::into)
    }

    fn get_random_existing_account(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<(S::Address, Self::AccountView)> {
        if self.accounts.is_empty() {
            self.generate_account(u)
        } else {
            let (address, account) = self.accounts.random_entry(u)?;
            Ok((address.clone(), account.into()))
        }
    }

    fn get_random_existing_account_with_tag(
        &mut self,
        tag: impl Into<Self::Tag>,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<Option<(<S as Spec>::Address, Self::AccountView)>> {
        if let Some(accounts) = self.tags.get(&tag.into()) {
            if accounts.is_empty() {
                return Ok(None);
            }
            let address = accounts.random_entry(u)?;
            let account = self
                .accounts
                .get(address)
                .expect("Account from secondary index must exist");
            Ok(Some((address.clone(), account.into())))
        } else {
            Ok(None)
        }
    }

    fn update_account<Tag: Into<Self::Tag>>(
        &mut self,
        address: S::Address,
        account_view: Self::AccountView,
        tags: Vec<TagAction<Tag>>,
    ) {
        assert!(
            self.accounts
                .get_mut(&address)
                .map(|acct| account_view.apply_to(acct))
                .is_some(),
            "Tried to update account that doesn't exist"
        );

        for action in tags {
            match action {
                TagAction::Add(tag) => {
                    self.tags
                        .entry(tag.into())
                        .or_default()
                        .insert(address.clone());
                }
                TagAction::Remove(tag) => {
                    self.tags
                        .get_mut(&tag.into())
                        .map(|set| set.swap_remove(&address));
                }
            }
        }
    }

    fn generate_account(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<(S::Address, Self::AccountView)> {
        let private_key: <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey =
            Arbitrary::arbitrary(u)?;
        let address: S::Address = (&private_key.pub_key()).into();
        let account = AccountState {
            balances: Vec::new(),
            can_mint: Default::default(),
            sequencing_bond: None,
            private_key,
            additional_info: Default::default(),
        };
        let view = (&account).into();
        self.accounts.insert(address.clone(), account);
        Ok((address, view))
    }
}
