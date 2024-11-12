use std::collections::HashMap;
use std::marker::PhantomData;

use arbitrary::Arbitrary;
use derivative::Derivative;
use indexmap::{IndexMap, IndexSet};
use sov_modules_api::prelude::arbitrary;

use super::interface::{CallMessageGenerator, DefaultEmpty, GeneratorState, PickRandom, TagAction};
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

impl<S: Spec, T: Default> AccountState<S, T> {
    pub fn with_private_key(private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey) -> Self {
        Self {
            balances: Vec::new(),
            can_mint: Default::default(),
            sequencing_bond: None,
            private_key,
            additional_info: Default::default(),
        }
    }
}
#[derive(Clone, Debug, Derivative)]
#[derivative(Default(bound = ""))]
pub struct AccountStateView<S: Spec, T = ()> {
    pub balances: Option<Vec<Coins>>,
    pub can_mint: Option<IndexSet<TokenId>>,
    pub sequencing_bond: Option<Option<u64>>,
    pub private_key: Option<<S::CryptoSpec as CryptoSpec>::PrivateKey>,
    pub additional_info: Option<T>,
}

macro_rules! apply {
    ($input:expr =>  $target:ident.$field:ident) => {
        if let Some(item) = $input {
            $target.$field = item;
        }
    };
}

impl<'a, S: Spec, T: Clone> From<&'a AccountState<S, T>> for AccountStateView<S, T> {
    fn from(value: &'a AccountState<S, T>) -> Self {
        Self {
            balances: Some(value.balances.clone()),
            can_mint: Some(value.can_mint.clone()),
            sequencing_bond: Some(value.sequencing_bond),
            private_key: Some(value.private_key.clone()),
            additional_info: Some(value.additional_info.clone()),
        }
    }
}

// TODO: Handle the generic of AccountStateView being a type that implements
// DefaultEmpty + ApplyTo<T> instead of literal T.
impl<S: Spec, T> ApplyTo<AccountState<S, T>> for AccountStateView<S, T> {
    fn apply_to(self, account: &mut AccountState<S, T>) {
        let AccountStateView {
            balances,
            can_mint,
            sequencing_bond,
            private_key,
            additional_info,
        } = self;
        apply!(balances  =>  account.balances);
        apply!(can_mint  =>  account.can_mint);
        apply!(sequencing_bond  =>  account.sequencing_bond);
        apply!(private_key  =>  account.private_key);
        apply!(additional_info  =>  account.additional_info);
    }
}

impl<S: Spec, T> DefaultEmpty for AccountStateView<S, T> {}

impl<S: Spec, T> From<&AccountState<S, T>> for BankAccount<S> {
    fn from(value: &AccountState<S, T>) -> BankAccount<S> {
        BankAccount {
            private_key: value.private_key.clone(),
            balances: value.balances.clone(),
            can_mint: value.can_mint.clone(),
        }
    }
}

impl<S: Spec, T> From<&AccountStateView<S, T>> for BankAccount<S> {
    fn from(value: &AccountStateView<S, T>) -> BankAccount<S> {
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
        }
    }
}

impl<S: Spec, T> ApplyTo<AccountStateView<S, T>> for BankAccount<S> {
    fn apply_to(self, account: &mut AccountStateView<S, T>) {
        account.balances = Some(self.balances);
        account.can_mint = Some(self.can_mint);
    }
}

impl<S: Spec, T> ApplyTo<AccountState<S, T>> for BankAccount<S> {
    fn apply_to(self, account: &mut AccountState<S, T>) {
        assert_eq!(
            account.private_key.pub_key(),
            self.private_key.pub_key(),
            "Applied to wrong account!"
        );
        account.balances = self.balances;
        account.can_mint = self.can_mint;
    }
}

/// Allows a state view to update the global state.
pub trait ApplyTo<T> {
    /// Applies any changes to a view onto the global account state
    fn apply_to(self, account: &mut T);
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

    pub fn with_account_and_tags(account: AccountState<S, T>, tags: Vec<M::Tag>) -> Self {
        let mut output = Self::default();
        let address: <S as Spec>::Address = (&account.private_key.pub_key()).into();
        for tag in tags {
            output.tags.entry(tag).or_default().insert(address.clone());
        }
        output.accounts.insert(address, account);

        output
    }
}

impl<S: Spec, M: CallMessageGenerator<S>, T: Default + 'static> GeneratorState<S> for State<S, M, T>
where
    for<'a> M::AccountView: From<&'a AccountState<S, T>> + ApplyTo<AccountState<S, T>>,
    // AccountState<S, T>: ApplyTo>,
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

    fn update_account(
        &mut self,
        address: S::Address,
        view: Self::AccountView,
        tags: Vec<TagAction<Self::Tag>>,
    ) {
        assert!(
            self.accounts
                .get_mut(&address)
                .map(|account| view.apply_to(account))
                .is_some(),
            "Tried to update account that doesn't exist"
        );

        for action in tags {
            match action {
                TagAction::Add(tag) => {
                    self.tags.entry(tag).or_default().insert(address.clone());
                }
                TagAction::Remove(tag) => {
                    self.tags.get_mut(&tag).map(|set| set.swap_remove(&address));
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
        let account = AccountState::<S, T>::with_private_key(private_key);
        let view = (&account).into();
        self.accounts.insert(address.clone(), account);
        Ok((address, view))
    }
}
