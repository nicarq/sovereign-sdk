use std::marker::PhantomData;

use sov_bank::{Coins, TokenId};
use sov_modules_api::prelude::arbitrary;
use sov_modules_api::prelude::arbitrary::Arbitrary;
use sov_modules_api::{CryptoSpec, Spec};
use tracing::warn;
mod transfer;

use crate::module_message_generators::interface::{
    CallMessageGenerator, GeneratedMessage, GeneratorState, MessageValidity, Percent, RandomUniform,
};

/// A generator for bank call messages.
pub struct BankMessageGenerator<S> {
    // The fraction of each callmessage type to generate.
    // These percentages must sum to 100.
    percent_mint: Percent,
    percent_transfer: Percent,
    percent_create_token: Percent,
    percent_freeze: Percent,

    // The fraction of valid messages that should create a new address. This may be
    // any valid between 0 and 100.
    address_creation_rate: Percent,

    phantom: PhantomData<S>,
}

impl<S> BankMessageGenerator<S> {
    pub fn new(
        percent_mint: Percent,
        percent_transfer: Percent,
        percent_create_token: Percent,
        percent_freeze: Percent,
        address_creation_rate: Percent,
    ) -> Self {
        assert!(percent_mint + percent_transfer + percent_create_token + percent_freeze == 100);

        Self {
            percent_mint,
            percent_transfer,
            percent_create_token,
            percent_freeze,
            address_creation_rate,
            phantom: PhantomData,
        }
    }
}

/// A complete description of any possible state change created by the bank message generator.
#[derive(Debug)]
pub enum BankChangeLogEntry<S: Spec> {
    BalanceChanged { address: S::Address, coins: Coins },
    // More variants will be added in coming PRs.
}

impl<S: Spec> BankChangeLogEntry<S> {
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

    type RollupStateReader = ();

    type ChangelogEntry = BankChangeLogEntry<S>;

    fn generate_call_message(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
        rollup_state_accessor: &Self::RollupStateReader,
        generator_state: &mut impl GeneratorState<S, AccountView = Self::AccountView>,
        validity: MessageValidity,
    ) -> arbitrary::Result<GeneratedMessage<S, Self::CallMessage, Self::ChangelogEntry>> {
        let kind = Percent::arbitrary(u)?;
        if kind < self.percent_transfer {
            match self
                .generate_transfer(u, rollup_state_accessor, generator_state, validity)
                .try_to_arbitrary()
            {
                Ok(transfer_result) => transfer_result,
                Err(e) => {
                    warn!(
                        "Failed to generate transfer: {:?}. Generating mint instead",
                        e
                    );
                    todo!()
                }
            }
        } else {
            todo!()
        }
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

/// The view of an account used by the bank message generator
pub struct BankAccount<S: Spec> {
    pub private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
    pub balances: Vec<Coins>,
}

impl<S: Spec> BankAccount<S> {
    /// Increments the balance in place. Returns a copy of the new balance.
    pub fn increment_balance(&mut self, coins: Coins) -> u64 {
        let Coins { amount, token_id } = coins;
        let balance = self.find_or_insert(token_id);
        balance.amount += amount;
        balance.amount
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
        let idx = usize::less_than(&self.balances.len(), u)?;
        Ok(self.balances.get_mut(idx))
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
