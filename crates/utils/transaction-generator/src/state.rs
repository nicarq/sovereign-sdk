//! Provides a basic implementation of the [`GeneratorState`] trait.
use std::collections::HashMap;
use std::marker::PhantomData;

use arbitrary::Arbitrary;
use derivative::Derivative;
use indexmap::{IndexMap, IndexSet};
use sov_bank::{config_gas_token_id, Amount, Coins, TokenId};
use sov_modules_api::prelude::arbitrary;
use sov_modules_api::{CryptoSpec, PrivateKey, Spec};

use super::interface::{CallMessageGenerator, DefaultEmpty, GeneratorState, PickRandom, TagAction};
use crate::generators::bank::{BankAccount, Tag as BankTag};
use crate::generators::value_setter::{Tag as ValueSetterTag, ValueSetterAccount};
use crate::interface::Taggable;

/// The state of an account in the message generator.
///
/// AccountState is generic over an `additional_state` field
/// to allow customization for external modules.
#[derive(Clone, Debug)]
pub struct AccountState<S: Spec, T = ()> {
    /// The token ID and amount of all known tokens for which this account has non-zero balance
    ///
    /// Note that tokens may exist which the transaction generator is *not* aware of.
    pub balances: Vec<Coins>,
    /// The set of known tokens which this account is allowed to mint
    pub can_mint: IndexSet<TokenId>,
    /// The bond amount that this account has locked in the sequencer registry, if applicable
    pub sequencing_bond: Option<u64>,
    /// The private key for this account
    pub private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
    /// Any additional state tracked by external modules
    pub additional_info: T,
}

impl<S: Spec, T: Default> AccountState<S, T> {
    /// Create an empty account with the given private key
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

/// A view into `AccountState` containing some subset of its data. Identical to `AccountState` except that all fields
/// are wrapped in an `Option` so that irrelevant fields can be ignored.
#[derive(Clone, Debug, Derivative)]
#[derivative(Default(bound = ""))]
pub struct AccountStateView<S: Spec, Tag, Data = ()> {
    /// The account's balances
    pub balances: Option<Vec<Coins>>,
    /// The set of known tokens which this account is allowed to mint
    pub can_mint: Option<IndexSet<TokenId>>,
    /// The bond amount that this account has locked in the sequencer registry, if applicable
    pub sequencing_bond: Option<Option<u64>>,
    /// The private key for this account
    pub private_key: Option<<S::CryptoSpec as CryptoSpec>::PrivateKey>,
    /// Any additional state tracked by external modules
    pub additional_info: Option<Data>,
    /// The tag changes for this account.
    pub tag_changes: Vec<TagAction<Tag>>,
}

impl<S: Spec, Tag: From<BankTag>, Data> From<BankAccount<S>> for AccountStateView<S, Tag, Data> {
    fn from(value: BankAccount<S>) -> Self {
        Self {
            balances: Some(value.balances),
            can_mint: Some(value.can_mint),
            sequencing_bond: None,
            private_key: Some(value.private_key),
            additional_info: None,
            tag_changes: value
                .tag_changes
                .into_iter()
                .map(|t| t.map(|tag| tag.into()))
                .collect(),
        }
    }
}

impl<S: Spec, Tag: From<ValueSetterTag>, Data> From<ValueSetterAccount<S>>
    for AccountStateView<S, Tag, Data>
{
    fn from(value: ValueSetterAccount<S>) -> Self {
        Self {
            balances: None,
            can_mint: None,
            sequencing_bond: None,
            private_key: Some(value.private_key),
            additional_info: None,
            tag_changes: vec![],
        }
    }
}

impl<'a, S: Spec, Tag, Data: Clone> From<&'a AccountState<S, Data>>
    for AccountStateView<S, Tag, Data>
{
    fn from(value: &'a AccountState<S, Data>) -> Self {
        Self {
            balances: Some(value.balances.clone()),
            can_mint: Some(value.can_mint.clone()),
            sequencing_bond: Some(value.sequencing_bond),
            private_key: Some(value.private_key.clone()),
            additional_info: Some(value.additional_info.clone()),
            tag_changes: Vec::new(),
        }
    }
}

macro_rules! apply {
    ($input:expr =>  $target:ident.$field:ident) => {
        if let Some(item) = $input {
            $target.$field = item;
        }
    };
}

// TODO: Handle the generic of AccountStateView being a type that implements
// DefaultEmpty + ApplyTo<T> instead of literal T.
impl<S: Spec, Tag, Data> ApplyTo<AccountState<S, Data>> for AccountStateView<S, Tag, Data> {
    fn apply_to(self, account: &mut AccountState<S, Data>) {
        let AccountStateView {
            balances,
            can_mint,
            sequencing_bond,
            private_key,
            additional_info,
            tag_changes,
        } = self;
        assert!(tag_changes.is_empty(), "When applying a view to global account state, tags must be handled separately to prevent data loss");
        apply!(balances  =>  account.balances);
        apply!(can_mint  =>  account.can_mint);
        apply!(sequencing_bond  =>  account.sequencing_bond);
        apply!(private_key  =>  account.private_key);
        apply!(additional_info  =>  account.additional_info);
    }
}

impl<S: Spec, Tag, Data> DefaultEmpty for AccountStateView<S, Tag, Data> {}

impl<S: Spec, Tag, Data> Taggable for AccountStateView<S, Tag, Data> {
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

/// Allows a state view to update the global state.
pub trait ApplyTo<T> {
    /// Applies any changes to a view onto the global account state
    fn apply_to(self, account: &mut T);
}

/// The token info needed for message generation
#[derive(Debug, Clone)]
pub struct TokenInfo {
    /// The current supply of the token
    pub total_supply: Amount,
}

/// A simple implementation of the [`GeneratorState`] trait. Tracks accounts by address
/// and maintains a secondary index using the `tags` provided by the module.
pub struct State<S: Spec, M: CallMessageGenerator<S>, T = ()> {
    accounts: IndexMap<S::Address, AccountState<S, T>>,
    tags: HashMap<M::Tag, IndexSet<S::Address>>,
    tokens: IndexMap<TokenId, TokenInfo>,
    phantom_module: PhantomData<M>,
}

impl<S: Spec, M: CallMessageGenerator<S>, T> Default for State<S, M, T> {
    fn default() -> Self {
        Self {
            accounts: Default::default(),
            phantom_module: Default::default(),
            tokens: Default::default(),
            tags: Default::default(),
        }
    }
}

impl<S: Spec, M: CallMessageGenerator<S>, T> State<S, M, T> {
    /// Create an empty [`State`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new state containing the provided account. Tags the account with the provided tags
    /// and intitializes the token_supply tracker for any relevant tokens. This method assumes that
    /// the account holder is the *only* holder of any tokens. If that assumption is violated, message
    /// generation may fail.
    pub fn with_account_and_tags(account: AccountState<S, T>, tags: Vec<M::Tag>) -> Self {
        let mut output = Self::default();
        let address: <S as Spec>::Address = (&account.private_key.pub_key()).into();
        for tag in tags {
            output.tags.entry(tag).or_default().insert(address.clone());
        }
        for balance in account.balances.iter() {
            let duplicate = output.tokens.insert(
                balance.token_id,
                TokenInfo {
                    total_supply: balance.amount,
                },
            );
            if balance.token_id == config_gas_token_id() {
                tracing::warn!("Using gas token for bank messsage generation. This can cause unexpected behavior because of gas payments!");
            }
            assert!(
                duplicate.is_none(),
                "Duplicate balances in initial account state"
            );
        }
        output.accounts.insert(address, account);

        output
    }
}

impl<S: Spec, M: CallMessageGenerator<S>, T: Default + 'static> GeneratorState<S> for State<S, M, T>
where
    for<'a> M::AccountView: From<&'a AccountState<S, T>>
        + ApplyTo<AccountState<S, T>>
        + Taggable<Tag = <M as CallMessageGenerator<S>>::Tag>,
{
    type AccountView = M::AccountView;

    type Tag = M::Tag;

    fn get_account(&self, address: &S::Address) -> Option<Self::AccountView> {
        self.accounts.get(address).map(Into::into)
    }

    fn get_account_with_tag(&self, tag: Self::Tag) -> Option<Self::AccountView> {
        let address = self.tags.get(&tag).and_then(|set| set.first());

        address.and_then(|address: &S::Address| self.get_account(address))
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

    fn update_account(&mut self, address: S::Address, mut view: Self::AccountView) {
        for action in view.take_tags().into_iter() {
            match action {
                TagAction::Add(tag) => {
                    self.tags.entry(tag).or_default().insert(address.clone());
                }
                TagAction::Remove(tag) => {
                    self.tags.get_mut(&tag).map(|set| set.swap_remove(&address));
                }
            }
        }
        assert!(
            self.accounts
                .get_mut(&address)
                .map(|account| view.apply_to(account))
                .is_some(),
            "Tried to update account that doesn't exist"
        );
    }

    fn has_tag(&mut self, addr: &S::Address, tag: impl Into<Self::Tag>) -> bool {
        self.tags
            .get(&tag.into())
            .map(|tag_holders| tag_holders.contains(addr))
            .unwrap_or(false)
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

    fn get_token(&mut self, id: &TokenId) -> Option<TokenInfo> {
        self.tokens.get(id).cloned()
    }

    fn update_token(&mut self, id: TokenId, info: TokenInfo) {
        self.tokens.insert(id, info);
    }

    fn get_random_token(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<(TokenId, TokenInfo)> {
        self.tokens.random_entry(u).map(|(k, v)| (*k, v.clone()))
    }
}
