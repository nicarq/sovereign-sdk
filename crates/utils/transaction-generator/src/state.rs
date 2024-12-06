//! Provides a basic implementation of the [`GeneratorState`] trait.
use std::collections::HashMap;

use arbitrary::Arbitrary;
use indexmap::{IndexMap, IndexSet};
use sov_bank::{config_gas_token_id, Amount, Coins, TokenId};
use sov_modules_api::prelude::arbitrary;
use sov_modules_api::{CryptoSpec, PrivateKey, Spec};

use super::interface::{GeneratorState, PickRandom, TagAction};
use crate::generators::basic::Tag;
use crate::interface::Taggable;

/// A view into `AccountState` containing some subset of its data. Identical to `AccountState` except that all fields
/// are wrapped in an `Option` so that irrelevant fields can be ignored.
#[derive(Clone, Debug)]
pub struct AccountState<S: Spec, Tag, Data = ()> {
    /// The account's balances
    pub balances: Vec<Coins>,
    /// The set of known tokens which this account is allowed to mint
    pub can_mint: IndexSet<TokenId>,
    /// The bond amount that this account has locked in the sequencer registry, if applicable
    pub sequencing_bond: Option<u64>,
    /// The private key for this account
    pub private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
    /// Any additional state tracked by external modules
    pub additional_info: Data,
    /// The tag changes for this account.
    pub tag_changes: Vec<TagAction<Tag>>,
}

impl<S: Spec, Tag, T: Default> AccountState<S, Tag, T> {
    /// Create an empty account with the given private key
    pub fn with_private_key(private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey) -> Self {
        Self {
            balances: Vec::new(),
            can_mint: Default::default(),
            sequencing_bond: None,
            private_key,
            additional_info: Default::default(),
            tag_changes: Vec::new(),
        }
    }
}

impl<S: Spec, Tag, Data> ApplyTo<AccountState<S, Tag, Data>> for AccountState<S, Tag, Data> {
    fn apply_to(self, account: &mut AccountState<S, Tag, Data>) {
        assert_eq!(
            self.private_key.pub_key(),
            account.private_key.pub_key(),
            "Public keys must match"
        );

        account.balances = self.balances;
        account.can_mint = self.can_mint;
        account.sequencing_bond = self.sequencing_bond;
        account.tag_changes = self.tag_changes;
        account.additional_info = self.additional_info;
    }
}

impl<S: Spec, Tag, Data> Taggable for AccountState<S, Tag, Data> {
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
pub struct State<S: Spec, T = ()> {
    accounts: IndexMap<S::Address, AccountState<S, Tag, T>>,
    tags: HashMap<Tag, IndexSet<S::Address>>,
    tokens: IndexMap<TokenId, TokenInfo>,
}

impl<S: Spec, T> Default for State<S, T> {
    fn default() -> Self {
        Self {
            accounts: Default::default(),
            tokens: Default::default(),
            tags: Default::default(),
        }
    }
}

impl<S: Spec, T> State<S, T> {
    /// Create an empty [`State`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new state containing the provided account. Tags the account with the provided tags
    /// and intitializes the token_supply tracker for any relevant tokens. This method assumes that
    /// the account holder is the *only* holder of any tokens. If that assumption is violated, message
    /// generation may fail.
    pub fn with_account_and_tags(account: AccountState<S, Tag, T>, tags: Vec<Tag>) -> Self {
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

impl<S: Spec, T: Default + 'static + Clone> GeneratorState<S> for State<S, T> {
    type AccountView = AccountState<S, Tag, T>;

    type Tag = Tag;

    fn get_account(&self, address: &S::Address) -> Option<Self::AccountView> {
        self.accounts.get(address).cloned()
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
            self.generate_account(u)?;
        }

        let (address, account) = self.accounts.random_entry(u)?;
        Ok((address.clone(), account.clone()))
    }

    fn get_random_existing_account_with_tag(
        &self,
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
            Ok(Some((address.clone(), account.clone())))
        } else {
            Ok(None)
        }
    }

    fn update_account(&mut self, address: &S::Address, mut view: Self::AccountView) {
        for action in view.take_tags().into_iter() {
            match action {
                TagAction::Add(tag) => {
                    self.tags.entry(tag).or_default().insert(address.clone());
                }
                TagAction::Remove(tag) => {
                    self.tags.get_mut(&tag).map(|set| set.swap_remove(address));
                }
            }
        }
        assert!(
            self.accounts
                .get_mut(address)
                .map(|account| view.apply_to(account))
                .is_some(),
            "Tried to update account that doesn't exist"
        );
    }

    fn has_tag(&self, addr: &S::Address, tag: impl Into<Self::Tag>) -> bool {
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
        let account = AccountState::<S, Tag, T>::with_private_key(private_key);
        self.accounts.insert(address.clone(), account.clone());
        Ok((address, account))
    }

    fn get_token(&self, id: &TokenId) -> Option<TokenInfo> {
        self.tokens.get(id).cloned()
    }

    fn update_token(&mut self, id: TokenId, info: TokenInfo) {
        self.tokens.insert(id, info);
    }

    fn get_random_token(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<(TokenId, TokenInfo)> {
        self.tokens.random_entry(u).map(|(k, v)| (*k, v.clone()))
    }
}
