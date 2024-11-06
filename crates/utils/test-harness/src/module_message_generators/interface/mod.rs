mod rng;
use std::hash::Hash;

use derive_more::derive::AsRef;
use derive_more::{Add, Mul};
pub use rng::*;
use sov_modules_api::prelude::arbitrary::{self, Arbitrary};
use sov_modules_api::Spec;

/// Whether a generated message should be valid or invalid.
#[derive(strum::EnumIs, Clone, Copy, PartialEq, Eq)]
pub enum MessageValidity {
    Valid,
    Invalid,
}

/// A generated message for a particular module.
#[derive(Debug)]
pub struct GeneratedMessage<S: Spec, M, E> {
    pub message: M,
    pub sender: S::Address,
    pub changes: Vec<E>,
}

impl<S: Spec, M, E> GeneratedMessage<S, M, E> {
    pub fn new(message: M, sender: S::Address, changes: Vec<E>) -> Self {
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
/// use arbitrary::Arbitrary;
///# use sov_test_harness::module_message_generators::interface::Percent;
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
    pub const fn one_hundred() -> Self {
        Self(100)
    }

    pub const fn fifty() -> Self {
        Self(50)
    }

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

#[derive(thiserror::Error, PartialEq, Eq, Clone, Hash, Debug)]
pub enum InvalidDistribution {
    #[error("Invalid distribution! Items must sum to 100 but were only: {0}")]
    TotalTooLow(u8),
    #[error("Invalid distribution! Total may not exceed 100.")]
    TotalTooHigh,
}

/// A distribution of probabilities, expressed as relative weights. Optionally, the
/// distribution may have associated values. This makes it easier to keep the weights
/// and values in sync
///
/// # Examples
///
/// ```rust
///# use sov_test_harness::module_message_generators::interface::Distribution;
///
/// Distribution::new([1, 1, 1]); // Selects each of the three possibilities one third of the time
/// Distribution::new([3, 1, 6]); // Assigns 30%, 10%, and 60% probabilities to each of three possibilities
/// Distribution::new([3, 1, 6]); // Assigns 30%, 10%, and 60% probabilities to each of three possibilities
/// ```
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
/// # use sov_test_harness::module_message_generators::bank::message_generator::BankAccount;
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
    type AccountView;
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
    fn update_account<T: Into<Self::Tag>>(
        &mut self,
        address: S::Address,
        account: Self::AccountView,
        tags: Vec<TagAction<T>>,
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
        if Percent::arbitrary(u)? <= generation_probability {
            self.generate_account(u)
        } else {
            self.get_random_existing_account(u)
        }
    }
}

pub enum TagAction<T> {
    Add(T),
    Remove(T),
}

pub trait CallMessageGenerator<S: Spec> {
    /// The module callmessage being generated.
    type CallMessage;

    /// The tag type used by this module, if applicable
    type Tag: Hash + Eq;

    /// The view of account state used by the module message generator
    type AccountView;

    /// A service which returns the current rollup state.
    type RollupStateReader;

    /// The relevant post state from a generatd message.
    type ChangelogEntry;

    /// Generates a `CallMessage`, potentially valid or invalid, based on the provided parameters.
    fn generate_call_message(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
        rollup_state_accessor: &Self::RollupStateReader,
        generator_state: &mut impl GeneratorState<
            S,
            AccountView = Self::AccountView,
            Tag: From<Self::Tag>,
        >,
        validity: MessageValidity,
    ) -> arbitrary::Result<GeneratedMessage<S, Self::CallMessage, Self::ChangelogEntry>>;

    /// Assert that the every value in the generator state matches the rollup state.
    fn assert_full_state(
        &self,
        rollup_state_accessor: &Self::RollupStateReader,
        generator_state: &mut impl GeneratorState<S, AccountView = Self::AccountView>,
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
