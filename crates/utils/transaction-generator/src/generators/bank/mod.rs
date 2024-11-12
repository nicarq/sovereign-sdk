//! Implements call message generation for the [`sov_bank::Bank`] module.
use std::marker::PhantomData;

use indexmap::IndexSet;
use sov_bank::{CallMessage, CallMessageDiscriminants, Coins, TokenId};
use sov_modules_api::prelude::arbitrary;
use sov_modules_api::{CryptoSpec, Spec};
use strum::VariantArray;
use tracing::warn;
mod mint;
mod transfer;

use crate::interface::{
    CallMessageGenerator, Distribution, GeneratedMessage, GeneratorState, MessageValidity, Percent,
    PickRandomMut,
};

/// The call message discriminants used by the `Bank` module
pub const MESSAGES: &[sov_bank::CallMessageDiscriminants] =
    sov_bank::CallMessageDiscriminants::VARIANTS;

/// A generator for bank call messages.
pub struct BankMessageGenerator<S> {
    message_distribution: Distribution<{ MESSAGES.len() }, CallMessageDiscriminants>,
    // The fraction of valid messages that should create a new address. This may be
    // any valid percent from 0 to 100 (inclusive).
    address_creation_rate: Percent,
    phantom: PhantomData<S>,
}

/// Configuration for the [`BankMessageGenerator`]
#[derive(Debug, Clone)]
pub struct BankMessageGeneratorConfig {
    /// The distribution of messages to generate
    pub message_distribution: Distribution<{ MESSAGES.len() }, CallMessageDiscriminants>,
    /// The fraction of valid callmessages which should generate new addresses. This parameter
    /// is used on a best-effort basis; if some kind of call message cannot create a new address,
    /// the actual percentage will be less than requested.
    pub address_creation_rate: Percent,
}

impl<S: Spec> BankMessageGenerator<S> {
    /// Instantiate a new [`BankMessageGenerator`]
    pub fn new(
        message_distribution: Distribution<{ MESSAGES.len() }, CallMessageDiscriminants>,
        address_creation_rate: Percent,
    ) -> Self {
        Self {
            message_distribution,
            address_creation_rate,
            phantom: PhantomData,
        }
    }

    /// Instantiate a new [`BankMessageGenerator`] from the provided config
    pub fn from_config(config: BankMessageGeneratorConfig) -> Self {
        Self::new(config.message_distribution, config.address_creation_rate)
    }

    /// Performs callmessage generation, falling back to variants that are more likely to succeed with limited state
    fn do_generation_with_fallback(
        &self,
        message_type: CallMessageDiscriminants,
        rollup_state_accessor: &(),
        u: &mut arbitrary::Unstructured<'_>,
        generator_state: &mut impl GeneratorState<S, AccountView = BankAccount<S>, Tag: From<Tag>>,
        validity: MessageValidity,
    ) -> InternalMessageGenResult<GeneratedMessage<S, CallMessage<S>, BankChangeLogEntry<S>>> {
        match message_type {
            CallMessageDiscriminants::Transfer => {
                match self
                    .generate_transfer(u, rollup_state_accessor, generator_state, validity)
                    .try_to_arbitrary()
                {
                    Ok(transfer_result) => Ok(transfer_result?),
                    Err(e) => {
                        warn!(
                            "Failed to generate transfer: {:?}. Generating mint instead",
                            e
                        );
                        self.do_generation_with_fallback(
                            CallMessageDiscriminants::Mint,
                            rollup_state_accessor,
                            u,
                            generator_state,
                            validity,
                        )
                    }
                }
            }
            CallMessageDiscriminants::CreateToken => todo!(),
            CallMessageDiscriminants::Burn => todo!(),
            CallMessageDiscriminants::Mint => {
                // TODO: Mint should fall back to create token
                match self
                    .generate_mint(u, rollup_state_accessor, generator_state, validity)
                    .try_to_arbitrary()
                {
                    Ok(transfer_result) => Ok(transfer_result?),
                    Err(e) => {
                        warn!("Failed to generate mint: {:?}", e);
                        todo!("Generate create token");
                    }
                }
            }
            CallMessageDiscriminants::Freeze => todo!(),
        }
    }
}

/// A complete description of any possible state change created by the bank message generator.
#[derive(Debug, Clone)]
pub enum BankChangeLogEntry<S: Spec> {
    /// The balance of an address changed
    BalanceChanged {
        #[allow(missing_docs)]
        address: S::Address,
        /// The balance after the change
        coins: Coins,
    },
    // More variants will be added in coming PRs.
}

impl<S: Spec> BankChangeLogEntry<S> {
    /// Creates a `BalanceChanged` [`BankChangeLogEntry`] and returns it
    pub fn balance_changed(address: S::Address, token_id: TokenId, new_balance: u64) -> Self {
        Self::BalanceChanged {
            address,
            coins: Coins {
                amount: new_balance,
                token_id,
            },
        }
    }
}

impl<S: Spec> CallMessageGenerator<S> for BankMessageGenerator<S> {
    type CallMessage = sov_bank::CallMessage<S>;

    type AccountView = BankAccount<S>;

    type Tag = Tag;

    type RollupStateReader = ();

    type ChangelogEntry = BankChangeLogEntry<S>;

    type Config = BankMessageGeneratorConfig;

    fn set_config(&mut self, config: Self::Config) {
        self.message_distribution = config.message_distribution;
        self.address_creation_rate = config.address_creation_rate;
    }

    fn generate_call_message(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
        rollup_state_accessor: &Self::RollupStateReader,
        generator_state: &mut impl GeneratorState<S, AccountView = Self::AccountView, Tag = Self::Tag>,
        validity: MessageValidity,
    ) -> arbitrary::Result<GeneratedMessage<S, Self::CallMessage, Self::ChangelogEntry>> {
        let message = *self.message_distribution.select_value(u)?;
        self.do_generation_with_fallback(
            message,
            rollup_state_accessor,
            u,
            generator_state,
            validity,
        )
        .try_to_arbitrary()
        .expect("Could not generate bank callmessage")
    }

    fn assert_full_state(
        &self,
        _rollup_state_accessor: &Self::RollupStateReader,
        _generator_state: &mut impl GeneratorState<S, AccountView = Self::AccountView>,
    ) -> Result<(), anyhow::Error> {
        todo!()
    }

    fn assert_incremental_state(
        &self,
        _rollup_state_accessor: &Self::RollupStateReader,
        _changes: Vec<Self::ChangelogEntry>,
    ) -> Result<(), anyhow::Error> {
        todo!()
    }
}

/// A tag used for indexing by the bank message generator
#[derive(PartialEq, Eq, Hash, Clone, Copy, Debug)]
pub enum Tag {
    /// Accounts which have a balance of some token. These can be used to generate transfers.
    HasBalance,
    /// Accounts which are allowed to mint some token.
    CanMint,
}
/// The view of an account used by the bank message generator
#[derive(Clone, Debug)]
pub struct BankAccount<S: Spec> {
    /// The account's private key
    pub private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
    /// All tokens of which the account has a non-zero balance
    pub balances: Vec<Coins>,
    /// The set of tokens that the account is allowed to mint
    pub can_mint: IndexSet<TokenId>,
}

impl<S: Spec> BankAccount<S> {
    /// Increments the balance in place. Returns a copy of the new balance.
    pub fn increment_balance(&mut self, coins: Coins) -> u64 {
        let Coins { amount, token_id } = coins;
        let balance = self.find_or_insert(token_id);
        balance.amount += amount;
        balance.amount
    }

    /// Find the balance of the given token
    pub fn balance_of(&self, token_id: TokenId) -> u64 {
        self.balances
            .iter()
            .find(|balance| balance.token_id == token_id)
            .map(|coins| coins.amount)
            .unwrap_or(0)
    }

    /// The maximum amount of the given token that can be received without overflowing
    pub fn receivable_balance(&self, token_id: TokenId) -> u64 {
        self.balances
            .iter()
            .find(|balance| balance.token_id == token_id)
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
    }

    /// Picks a balance at random from the balances list, if possible.
    pub fn pick_random_balance(
        &mut self,
        u: &mut arbitrary::Unstructured<'_>,
    ) -> arbitrary::Result<Option<&mut Coins>> {
        if self.balances.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.balances.random_entry_mut(u)?))
    }

    /// Return a reference to the balances entry for the given token, creating one
    /// with zero balance if necessary. Callers should be careful to delete the entry
    /// if they don't update the balance.
    fn find_or_insert(&mut self, token_id: TokenId) -> &mut Coins {
        // We use a somewhat convoluted method to get the correct balance by index here because the borrow checker
        // couldn't infer the correct lifetimes if we used iter_mut.
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

/// An error generated during message generation
#[derive(thiserror::Error, Debug)]
enum InternalMessageGenError {
    #[error(transparent)]
    Arbitrary(#[from] arbitrary::Error),
    /// A transfer could not be generated because no account with sufficient balance was found.
    // Note: If no account with balance can be found, we can simply try to generate
    // a create or mint token message.
    #[error("Could not find an account with balance to transfer")]
    NoAccountWithBalance,
    /// An invalid mint could not be generated because no account without appropriate permissions could be found
    #[error("Could not find an account that is *not* authorized to mint")]
    NoNonMintingAccounts,
    /// A mint could not be generated because no account without appropriate permissions could be found
    #[error("Could not find an account that is authorized to mint")]
    NoMintingAccounts,
    /// A mint could not be generated because no account could receive the token
    #[error("Could not find an account can receive {0}")]
    NoAccountsCanReceive(Coins),
}

type InternalMessageGenResult<T, E = InternalMessageGenError> = Result<T, E>;

/// Try to convert a result type *containing* `Arbitrary::Error` to an `arbitrary::Result`, failing
/// only if the type is `Err` *and* the contained error is not an arbitrary error.
trait TryToArbitrary<T>: Sized {
    fn try_to_arbitrary(self) -> Result<arbitrary::Result<T>, Self>;
}

impl<T> TryToArbitrary<T> for InternalMessageGenResult<T> {
    fn try_to_arbitrary(self) -> Result<arbitrary::Result<T>, Self> {
        match self {
            Ok(ok) => Ok(Ok(ok)),
            Err(InternalMessageGenError::Arbitrary(e)) => Ok(Err(e)),
            _ => Err(self),
        }
    }
}
