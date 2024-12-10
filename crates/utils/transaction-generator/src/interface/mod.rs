//! Defines the traits and types that form the interface for callmessage generation
mod rng;
use std::fmt::Debug;
use std::hash::Hash;
use std::marker::PhantomData;
pub mod traits;
use derive_more::derive::AsRef;
use derive_more::{Add, Mul};
pub use rng::*;
use sov_bank::TokenId;
use sov_modules_api::prelude::arbitrary::{self, Arbitrary};
use sov_modules_api::{CryptoSpec, PrivateKey, Spec};
pub use traits::*;

use super::state::ApplyToState;
use crate::state::{AccountState, State, TokenInfo};

/// Whether a generated message should be valid or invalid.
#[derive(strum::EnumIs, Clone, Copy, PartialEq, Eq)]
pub enum MessageValidity {
    #[allow(missing_docs)]
    Valid,
    #[allow(missing_docs)]
    Invalid,
}

impl MessageValidity {
    /// Make a distribution of message validity with the provided percentage of valid messages
    pub fn as_distribution(percentage_valid_messages: Percent) -> Distribution<2, MessageValidity> {
        Distribution::with_values(
            [MessageValidity::Valid, MessageValidity::Invalid],
            [
                percentage_valid_messages.0 as u64,
                (100 - percentage_valid_messages.0) as u64,
            ],
        )
    }
}

/// A generated message for a particular module.
#[derive(Debug, Clone)]
pub struct GeneratedMessage<S: Spec, M, E> {
    /// The generated call message
    pub message: M,
    /// The private key that should sign the message
    pub sender: <S::CryptoSpec as CryptoSpec>::PrivateKey,
    /// A summary of the changes that the transaction will make.
    pub changes: Vec<E>,
}

impl<S: Spec, M, E> GeneratedMessage<S, M, E> {
    /// Create a new [`GeneratedMessage`] from its components
    pub fn new(
        message: M,
        sender: <S::CryptoSpec as CryptoSpec>::PrivateKey,
        changes: Vec<E>,
    ) -> Self {
        Self {
            message,
            sender,
            changes,
        }
    }
}

/// A percentage, expressed as a whole number. Typically used to express the probability that some branch of the
/// generator will be taken. To decide whether to take a particular branch, use the pattern...
///
/// ```rust
///# use sov_modules_api::prelude::arbitrary;
///  use arbitrary::Arbitrary;
///# use sov_transaction_generator::interface::Percent;
///
///
/// fn should_take_branch<'a>(likelihood: Percent, u: &mut arbitrary::Unstructured<'a>) -> bool {
///     let random = Percent::arbitrary(u).unwrap();
///     if random < likelihood {
///         true
///     } else {
///         false
///     }
/// }
/// ```
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash, Debug, Add, Mul, AsRef)]
pub struct Percent(u8);

impl std::ops::Sub for Percent {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

impl Percent {
    /// Returns 100 percent
    pub const fn one_hundred() -> Self {
        Self(100)
    }

    /// Returns fifty percent
    pub const fn fifty() -> Self {
        Self(50)
    }

    /// Returns zero
    pub const fn zero() -> Self {
        Self(0)
    }
}

impl std::iter::Sum for Percent {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        let mut sum = 0;
        for item in iter {
            sum += item.0;
        }
        Percent(sum)
    }
}

impl<'a> Arbitrary<'a> for Percent {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(Self(u.int_in_range(0..=99)?))
    }
}

impl PartialEq<u8> for Percent {
    fn eq(&self, rhs: &u8) -> bool {
        self.0 == *rhs
    }
}

/// A distribution of probabilities, expressed as relative weights. Optionally, the
/// distribution may have associated values. This makes it easier to keep the weights
/// and values in sync
///
/// # Examples
///
/// ```rust
///# use sov_transaction_generator::interface::Distribution;
///
/// Distribution::new([1, 1, 1]); // Selects each of the three possibilities one third of the time
/// Distribution::new([3, 1, 6]); // Assigns 30%, 10%, and 60% probabilities to each of three possibilities
/// Distribution::new([3, 1, 6]); // Assigns 30%, 10%, and 60% probabilities to each of three possibilities
/// ```
#[derive(Debug, Clone)]
pub struct Distribution<const N: usize, T = ()> {
    values: [T; N],
    weights: [u64; N],
    sum: u128,
}

impl<const N: usize, T> Distribution<N, T> {
    /// Creates a new distribution with the given weights
    pub fn with_values(values: [T; N], weights: [u64; N]) -> Self {
        let sum = weights.iter().map(|v| *v as u128).sum();
        Self {
            values,
            weights,
            sum,
        }
    }

    /// Creates a new distribution with the given weights
    pub fn with_equiprobable_values(values: [T; N]) -> Self {
        Self::with_values(values, [1; N])
    }

    /// Pick from the distribution at random. Return a usize in range 0..N
    ///
    /// # Panics
    /// Panics if the number of provided choices is not `N`
    pub fn select_idx(&self, u: &mut arbitrary::Unstructured<'_>) -> arbitrary::Result<usize> {
        let mut target: u128 = u.int_in_range(1..=self.sum)?;
        for (idx, item) in self.weights.iter().cloned().enumerate() {
            if target <= (item as u128) {
                return Ok(idx);
            } else {
                target -= item as u128;
            }
        }
        unreachable!()
    }

    /// Pick from the values at random.
    pub fn select_value(&self, u: &mut arbitrary::Unstructured<'_>) -> arbitrary::Result<&T> {
        Ok(&self.values[self.select_idx(u)?])
    }
}

impl<const N: usize> Distribution<N> {
    /// Creates a new distribution with the given weights
    pub fn new(weights: [u64; N]) -> Self {
        Self::with_values([(); N], weights)
    }
    /// Select an entry at random from `choices` according to the probability distribution
    ///
    /// # Panics
    /// Panics if the number of provided choices is not `N`
    pub fn select_from<'a, T>(
        &self,
        choices: &'a [T],
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<&'a T> {
        Ok(&choices[self.select_idx(u)?])
    }

    /// Take all branches with equal probability
    pub fn equiprobable() -> Self {
        Self::new([1; N])
    }
}

/// Converts a more general `GeneratorState` implementation to a more specific one - for example,
/// the `GeneratorState` for a whole runtime to the state for a particular module
pub struct GeneratorStateMapper<'a, S: Spec, Acct, Tag, T = ()>(
    &'a mut State<S, Tag, T>,
    PhantomData<Acct>,
);

impl<'a, S: Spec, Acct, Tag, T> GeneratorStateMapper<'a, S, Acct, Tag, T> {
    /// Create  a new [`GeneratorStateMapper`]
    pub fn new(state: &'a mut State<S, Tag, T>) -> Self {
        Self(state, PhantomData)
    }
}

impl<'a, Acct, Tag, S: Spec, T: Default + Clone + 'static> GeneratorState<S>
    for GeneratorStateMapper<'a, S, Acct, Tag, T>
where
    Acct: Debug
        + Clone
        + for<'b> From<&'b AccountState<S, T>>
        + ApplyToState<S, T>
        + Taggable<Tag: Into<Tag>>,

    Tag: Eq + Hash + Debug + Clone,
{
    type AccountView = Acct;

    type Tag = Tag;

    fn get_account(&self, address: &S::Address) -> Option<Self::AccountView> {
        self.0.accounts.get(address).map(|acct| acct.into())
    }

    fn get_account_with_tag(&self, tag: Self::Tag) -> Option<Self::AccountView> {
        let address = self.0.tags.get(&tag).and_then(|set| set.first());

        address.and_then(|address: &S::Address| self.get_account(address))
    }

    fn get_random_existing_account(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<(S::Address, Self::AccountView)> {
        if self.0.accounts.is_empty() {
            self.generate_account(u)?;
        }

        let (address, account) = self.0.accounts.random_entry(u)?;
        Ok((address.clone(), account.into()))
    }

    fn get_random_existing_account_with_tag(
        &self,
        tag: Self::Tag,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<Option<(S::Address, Self::AccountView)>> {
        if let Some(accounts) = self.0.tags.get(&tag) {
            if accounts.is_empty() {
                return Ok(None);
            }
            let address = accounts.random_entry(u)?;
            let account = self
                .0
                .accounts
                .get(address)
                .expect("Account from secondary index must exist");
            Ok(Some((address.clone(), account.into())))
        } else {
            Ok(None)
        }
    }

    fn update_account(&mut self, address: &S::Address, mut view: Self::AccountView) {
        for action in view.take_tags().into_iter() {
            match action {
                TagAction::Add(tag) => {
                    self.0
                        .tags
                        .entry(tag.into())
                        .or_default()
                        .insert(address.clone());
                }
                TagAction::Remove(tag) => {
                    self.0
                        .tags
                        .get_mut(&tag.into())
                        .map(|set| set.swap_remove(address));
                }
            }
        }
        assert!(
            self.0
                .accounts
                .get_mut(address)
                .map(|account| view.apply_to(account))
                .is_some(),
            "Tried to update account that doesn't exist"
        );
    }

    fn has_tag(&self, addr: &<S as Spec>::Address, tag: Self::Tag) -> bool {
        self.0
            .tags
            .get(&tag)
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
        self.0.accounts.insert(address.clone(), account.clone());
        Ok((address, (&account).into()))
    }

    fn get_token(&self, id: &TokenId) -> Option<TokenInfo> {
        self.0.tokens.get(id).cloned()
    }

    fn update_token(&mut self, id: TokenId, info: TokenInfo) {
        self.0.tokens.insert(id, info);
    }

    fn get_random_token(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<(TokenId, TokenInfo)> {
        self.0.tokens.random_entry(u).map(|(k, v)| (*k, v.clone()))
    }
}

/// The action to take on a particular tag
#[derive(Debug, Clone)]
pub enum TagAction<T> {
    /// Add the tag to an account.
    Add(T),
    /// Remove the tag from an account, if present.
    Remove(T),
}

impl<T> TagAction<T> {
    /// Apply some function to the tag contained in some [`TagAction`]
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> TagAction<U> {
        match self {
            TagAction::Add(item) => TagAction::Add(f(item)),
            TagAction::Remove(item) => TagAction::Remove(f(item)),
        }
    }
}

/// Try to take an action repeatedly, breaking when the `until` condition is true and executing
/// the `on_failure` expression if unsuccessful after too many attempts.
#[macro_export]
macro_rules! repeatedly {
    (let $assignment:tt = $expression:expr; until: $test:expr, on_failure: $err:expr) => {
        let $assignment = 'repeated: {
            for _ in 0..1_000 {
                let $assignment = $expression;
                if $test {
                    break 'repeated $assignment;
                }
            }
            $err
        };
    };
}
