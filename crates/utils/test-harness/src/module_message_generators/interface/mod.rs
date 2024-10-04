mod rng;
use derive_more::{Add, Mul};
pub use rng::*;
use sov_modules_api::prelude::arbitrary::{self, Arbitrary};
use sov_modules_api::Spec;

/// Whether a generated message should be valid or invalid.
#[derive(strum::EnumIs)]
pub enum MessageValidity {
    Valid,
    Invalid,
}

/// A generated message for a particular module.
#[derive(Debug)]
pub struct GeneratedMessage<S: Spec, M, E> {
    message: M,
    sender: S::Address,
    changes: Vec<E>,
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
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash, Debug, Add, Mul)]
pub struct Percent(u8);

impl<'a> Arbitrary<'a> for Percent {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(Self(u8::less_than(&100, u)?))
    }
}

impl PartialEq<u8> for Percent {
    fn eq(&self, rhs: &u8) -> bool {
        self.0 == *rhs
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
///       }
///    }
/// }
/// ```
pub trait GeneratorState<S: Spec> {
    type AccountView;
    /// Creates a fresh copy of the appropriate view of the account with the given address.
    fn get_account(&self, address: S::Address) -> Option<Self::AccountView>;

    /// Picks an account at random from the generator state and returns a copy.
    fn get_random_existing_account(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<(S::Address, Self::AccountView)>;

    /// Updates the given account to match the provided view.
    fn update_account(&mut self, address: S::Address, account: Self::AccountView);

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

pub trait CallMessageGenerator<S: Spec> {
    /// The module callmessage being generated.
    type CallMessage;

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
        generator_state: &mut impl GeneratorState<S, AccountView = Self::AccountView>,
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
