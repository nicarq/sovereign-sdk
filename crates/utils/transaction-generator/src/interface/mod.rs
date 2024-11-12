//! Defines the traits and types that form the interface for callmessage generation
mod rng;
use std::fmt::Debug;
use std::hash::Hash;
use std::marker::PhantomData;

use derive_more::derive::AsRef;
use derive_more::{Add, Mul};
pub use rng::*;
use sov_modules_api::prelude::arbitrary::{self, Arbitrary};
use sov_modules_api::{CryptoSpec, Spec};

use super::state::ApplyTo;

/// Whether a generated message should be valid or invalid.
#[derive(strum::EnumIs, Clone, Copy, PartialEq, Eq)]
pub enum MessageValidity {
    #[allow(missing_docs)]
    Valid,
    #[allow(missing_docs)]
    Invalid,
}

/// A generated message for a particular module.
#[derive(Debug)]
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

/// The state of the transaction generator, which is shared across all modules.
///
/// The generator state maintains a global state, but partitions it into "views" containing
/// information relevant to a single module. For example, to test the "bank" and sequencer
/// registry modules, the global state would looks something like this:
///
/// ```rust
/// use sov_bank::Coins;
/// use sov_modules_api::{CryptoSpec, Spec};
/// # use sov_transaction_generator::generators::bank::BankAccount;
///
/// struct AccountState<S: Spec> {
///   pub balances: Vec<Coins>,
///   pub sequencing_bond: Option<u64>,
///   pub private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
/// }
///
/// impl<S: Spec> Into<BankAccount<S>> for &AccountState<S> {
///    fn into(self) -> BankAccount<S> {
///       BankAccount {
///          private_key: self.private_key.clone(),
///          balances: self.balances.clone(),
///          can_mint: Default::default(),
///       }
///    }
/// }
/// ```
pub trait GeneratorState<S: Spec> {
    /// The view of an account for a particular module
    type AccountView;

    /// The `Tag` enum associated with this generator. Tags form a secondary index
    /// that can be used to quickly recover accounts matching certain criteria.
    type Tag: Hash + Eq;

    /// Creates a fresh copy of the appropriate view of the account with the given address.
    fn get_account(&self, address: S::Address) -> Option<Self::AccountView>;

    /// Picks an account at random from the generator state and returns a copy.
    fn get_random_existing_account(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<(S::Address, Self::AccountView)>;

    /// Picks an account at random from the generator state and returns a copy.
    fn get_random_existing_account_with_tag(
        &mut self,
        tag: impl Into<Self::Tag>,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<Option<(S::Address, Self::AccountView)>>;

    /// Updates the given account to match the provided view.
    fn update_account(
        &mut self,
        address: S::Address,
        view: Self::AccountView,
        tags: Vec<TagAction<Self::Tag>>,
    );

    /// Generates an empty account and returns it.
    fn generate_account(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<(S::Address, Self::AccountView)>;

    /// Either picks or generates an account.
    fn get_or_generate(
        &mut self,
        generation_probability: Percent,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<(S::Address, Self::AccountView)> {
        if Percent::arbitrary(u)? < generation_probability {
            self.generate_account(u)
        } else {
            self.get_random_existing_account(u)
        }
    }
}

/// Converts a more general `GeneratorState` implementation to a more specific one - for example,
/// the `GeneratorState` for a whole runtime to the state for a particular module
pub struct GeneratorStateMapper<'a, Source, Acct, Tag>(&'a mut Source, PhantomData<(Acct, Tag)>);

impl<'a, Source, Acct, Tag> GeneratorStateMapper<'a, Source, Acct, Tag> {
    /// Create  a new [`GeneratorStateMapper`]
    pub fn new(state: &'a mut Source) -> Self {
        Self(state, PhantomData)
    }
}

/// A marker trait indicating that the `Default` implementation of a trait
/// corresponds to no change when applied to an Account
pub trait DefaultEmpty: Default {}

impl<'a, Source: GeneratorState<S, AccountView: DefaultEmpty>, Acct, Tag, S: Spec> GeneratorState<S>
    for GeneratorStateMapper<'a, Source, Acct, Tag>
where
    Acct:
        for<'acct> From<&'acct Source::AccountView> + ApplyTo<Source::AccountView> + Debug + Clone,

    Tag: Into<Source::Tag> + Eq + Hash + Debug + Clone,
{
    type AccountView = Acct;

    type Tag = Tag;

    fn get_account(&self, address: S::Address) -> Option<Self::AccountView> {
        self.0.get_account(address).map(|acct| (&acct).into())
    }

    fn get_random_existing_account(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<(S::Address, Self::AccountView)> {
        self.0
            .get_random_existing_account(u)
            .map(|(addr, acct)| (addr, (&acct).into()))
    }

    fn get_random_existing_account_with_tag(
        &mut self,
        tag: impl Into<Self::Tag>,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<Option<(S::Address, Self::AccountView)>> {
        Ok(self
            .0
            .get_random_existing_account_with_tag(tag.into(), u)?
            .map(|(addr, acct)| (addr, (&acct).into())))
    }

    fn update_account(
        &mut self,
        address: S::Address,
        view: Self::AccountView,
        tags: Vec<TagAction<Self::Tag>>,
    ) {
        let mut mapped = Source::AccountView::default();
        view.apply_to(&mut mapped);
        self.0.update_account(
            address,
            mapped,
            tags.into_iter().map(|x| x.map(Into::into)).collect(),
        );
    }

    fn generate_account(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<(S::Address, Self::AccountView)> {
        self.0
            .generate_account(u)
            .map(|(addr, acct)| (addr, (&acct).into()))
    }
}

/// Converts a type into a partial implementation of another type
/// without loss.
pub trait Updatewith<T> {
    /// Convert the item into a part-empty type.
    fn update_with(&mut self, view: T);
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

/// A standard interface for generating call messages and checking that they produce
/// the expected effects.
pub trait CallMessageGenerator<S: Spec> {
    /// The module callmessage being generated.
    type CallMessage;

    /// The tag type used by this module, if applicable
    type Tag: std::fmt::Debug + Hash + Eq;

    /// The view of account state used by the module message generator
    type AccountView: Clone + std::fmt::Debug;

    /// A service which returns the current rollup state.
    type RollupStateReader;

    /// The relevant post state from a generatd message.
    type ChangelogEntry: Clone + std::fmt::Debug;

    /// The config for this message generator.
    type Config: Clone + std::fmt::Debug;

    /// Updates the configuration of this generator
    fn set_config(&mut self, config: Self::Config);

    /// Generates a `CallMessage`, potentially valid or invalid, based on the provided parameters.
    fn generate_call_message(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
        rollup_state_accessor: &Self::RollupStateReader,
        generator_state: &mut impl GeneratorState<S, AccountView = Self::AccountView, Tag = Self::Tag>,
        validity: MessageValidity,
    ) -> arbitrary::Result<GeneratedMessage<S, Self::CallMessage, Self::ChangelogEntry>>;

    /// Assert that the every value in the generator state matches the rollup state.
    fn assert_full_state(
        &self,
        rollup_state_accessor: &Self::RollupStateReader,
        generator_state: &mut impl GeneratorState<S, AccountView = Self::AccountView, Tag = Self::Tag>,
    ) -> Result<(), anyhow::Error>;

    /// Assert that the rollup state matches the expected value. This method
    /// *must* detect when two changes conflict (if applicable) and assert only
    /// the most recent change.
    fn assert_incremental_state(
        &self,
        rollup_state_accessor: &Self::RollupStateReader,
        changes: Vec<Self::ChangelogEntry>,
    ) -> Result<(), anyhow::Error>;
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
