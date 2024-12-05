//! Implements call message generation for the [`sov_bank::Bank`] module.
use std::collections::HashSet;
use std::marker::PhantomData;
use std::sync::Arc;

use sov_bank::{Amount, CallMessage, CallMessageDiscriminants, Coins, TokenId};
use sov_modules_api::prelude::axum::async_trait;
use sov_modules_api::prelude::{arbitrary, tokio};
use sov_modules_api::Spec;
use strum::VariantArray;
use tracing::warn;
mod mint;
mod query;
pub use query::http::HttpBankClient;
pub use query::BankClient;
mod account;
mod create_token;
mod transfer;
pub use account::BankAccount;

use crate::interface::{
    CallMessageGenerator, Distribution, GeneratedMessage, GeneratorState, MessageValidity, Percent,
};

/// The call message discriminants used by the `Bank` module
pub const MESSAGES: &[sov_bank::CallMessageDiscriminants] =
    sov_bank::CallMessageDiscriminants::VARIANTS;

/// A generator for bank call messages.
#[derive(Debug, Clone)]
pub struct BankMessageGenerator<S> {
    message_distribution: Distribution<{ MESSAGES.len() }, CallMessageDiscriminants>,
    // The fraction of valid messages that should create a new address. This may be
    // any valid percent from 0 to 100 (inclusive).
    pub(crate) address_creation_rate: Percent,
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
        u: &mut arbitrary::Unstructured<'_>,
        generator_state: &mut impl GeneratorState<S, AccountView = BankAccount<S>, Tag = Tag>,
        validity: MessageValidity,
    ) -> InternalMessageGenResult<GeneratedMessage<S, CallMessage<S>, BankChangeLogEntry<S>>> {
        match message_type {
            CallMessageDiscriminants::Transfer => {
                match self
                    .generate_transfer(u, generator_state, validity)
                    .try_to_arbitrary()
                {
                    Ok(transfer_result) => Ok(transfer_result?),
                    Err(e) => {
                        warn!(
                            "Failed to generate transfer: {:?}. Generating mint instead",
                            e
                        );
                        assert!(
                            validity.is_valid(),
                            "Failed to generate an invalid transfer message. This should be unreachable, since generating *invalid* transfers is possible regardless of the rollup state."
                        );
                        self.do_generation_with_fallback(
                            CallMessageDiscriminants::Mint,
                            u,
                            generator_state,
                            validity,
                        )
                    }
                }
            }
            CallMessageDiscriminants::CreateToken => {
                match self
                    .generate_create_token(u, generator_state, validity)
                    .try_to_arbitrary()
                {
                    Ok(create_result) => Ok(create_result?),
                    Err(e) => {
                        warn!("Failed to generate create token: {:?}", e);
                        assert!(
                            validity.is_invalid(),
                            "Valid token creation message gen is infallible"
                        );
                        // Fall back to generating an *invalid* transfer, which is always possible
                        self.do_generation_with_fallback(
                            CallMessageDiscriminants::Transfer,
                            u,
                            generator_state,
                            validity,
                        )
                    }
                }
            }
            CallMessageDiscriminants::Burn => todo!(),
            CallMessageDiscriminants::Mint => {
                match self
                    .generate_mint(u, generator_state, validity)
                    .try_to_arbitrary()
                {
                    Ok(transfer_result) => Ok(transfer_result?),
                    Err(e) => {
                        warn!("Failed to generate mint: {:?}", e);
                        self.do_generation_with_fallback(
                            CallMessageDiscriminants::CreateToken,
                            u,
                            generator_state,
                            validity,
                        )
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

    /// The supply of a token changed
    SupplyChanged {
        #[allow(missing_docs)]
        token_id: TokenId,
        /// The total supply after the change
        total_supply: Amount,
    },
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

    /// Creates a [`BankChangeLogEntry::SupplyChanged`] and returns it
    pub fn supply_changed(token_id: TokenId, total_supply: Amount) -> Self {
        Self::SupplyChanged {
            token_id,
            total_supply,
        }
    }
}

#[async_trait]
impl<S: Spec> CallMessageGenerator<S> for BankMessageGenerator<S> {
    type CallMessage = sov_bank::CallMessage<S>;

    type AccountView = BankAccount<S>;

    type Tag = Tag;

    type RollupStateReader = HttpBankClient<S>;

    type ChangelogEntry = BankChangeLogEntry<S>;

    type Config = BankMessageGeneratorConfig;

    fn set_config(&mut self, config: Self::Config) {
        self.message_distribution = config.message_distribution;
        self.address_creation_rate = config.address_creation_rate;
    }

    fn generate_call_message(
        &self,
        u: &mut arbitrary::Unstructured<'_>,
        generator_state: &mut impl GeneratorState<S, AccountView = Self::AccountView, Tag = Self::Tag>,
        validity: MessageValidity,
    ) -> arbitrary::Result<GeneratedMessage<S, Self::CallMessage, Self::ChangelogEntry>> {
        let message = *self.message_distribution.select_value(u)?;
        self.do_generation_with_fallback(message, u, generator_state, validity)
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

    async fn assert_incremental_state(
        &self,
        rollup_state_accessor: Self::RollupStateReader,
        changes: Vec<Self::ChangelogEntry>,
    ) -> Result<(), anyhow::Error> {
        // Since newer changes will stomp older ones, we need to remember
        // which keys we've already checked.
        let mut checked_balances = HashSet::new();
        let mut checked_supplies = HashSet::new();
        let mut joinset = tokio::task::JoinSet::new();
        let accessor = Arc::new(rollup_state_accessor);
        for change in changes.into_iter().rev() {
            match change {
                BankChangeLogEntry::BalanceChanged { address, coins } => {
                    let Coins { token_id, amount } = coins;
                    let key = (address.clone(), token_id);
                    if checked_balances.contains(&key) {
                        continue;
                    } else {
                        checked_balances.insert(key);
                        let accessor = accessor.clone();
                        joinset.spawn(async move {
                            let found_balance = accessor.get_balance(&address, token_id).await;
                            assert_eq!(
                                found_balance, amount,
                                "Unexpected balance of {} at address {}",
                                token_id, &address
                            );
                        });
                    }
                }
                BankChangeLogEntry::SupplyChanged {
                    token_id,
                    total_supply,
                } => {
                    // HashSet::insert returns whether the value was newly inserted
                    if !checked_supplies.insert(token_id) {
                        continue;
                    }
                    let accessor = accessor.clone();
                    joinset.spawn(async move {
                        let found_supply = accessor.get_total_supply(&token_id).await;
                        assert_eq!(
                            found_supply, total_supply,
                            "Unexpected total supply of {}",
                            token_id,
                        );
                    });
                }
            }
        }
        while let Some(result) = joinset.join_next().await {
            result?;
        }

        Ok(())
    }
}

/// A tag used for indexing by the bank message generator
#[derive(PartialEq, Eq, Hash, Clone, Copy, Debug)]
pub enum Tag {
    /// Accounts which have a balance of some token. These can be used to generate transfers.
    HasBalance,
    /// Accounts which are allowed to mint some token.
    CanMint,
    /// Accounts which are allowed to mint some particular token.
    CanMintById(TokenId),
    /// Accounts which have created a token.
    HasCreatedToken,
}

/// An error generated during message generation
#[derive(thiserror::Error, Debug)]
pub(crate) enum InternalMessageGenError {
    #[error(transparent)]
    Arbitrary(#[from] arbitrary::Error),
    /// A transfer could not be generated because no account with sufficient balance was found.
    // Note: If no account with balance can be found, we can simply try to generate
    // a create or mint token message.
    #[error("Could not find an account with balance to transfer")]
    NoAccountWithBalance,
    /// An invalid mint could not be generated because no account without appropriate permissions could be found
    #[error("Could not find an account that is *not* authorized to mint")]
    NonMintingAccountNotFound,
    /// A mint could not be generated because no account without appropriate permissions could be found
    #[error("Could not find an account that is authorized to mint")]
    NoMintingAccounts,
    /// A mint could not be generated because no account could receive the token
    #[error("Could not find an account can receive {0}")]
    NoAccountsCanReceive(Coins),
    /// A create token can only fail if the account has already created a token with the same name,
    /// so generating an invalid `create_token` can fail in this case
    #[error("Could not find an account that has created a token")]
    NoAccountsHaveCreatedTokensYet,
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
