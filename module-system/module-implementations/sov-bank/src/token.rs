#[cfg(feature = "native")]
use core::str::FromStr;
use std::collections::HashSet;
use std::fmt::Formatter;
#[cfg(feature = "native")]
use std::num::ParseIntError;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use sov_modules_api::{impl_hash32_type, StateAccessor, WorkingSet};
use sov_state::Prefix;
#[cfg(feature = "native")]
use thiserror::Error;

use crate::call::prefix_from_address_with_parent;

/// Type alias to store an amount of token.
pub type Amount = u64;

impl_hash32_type!(TokenId, TokenIdBech32, "token_");

/// Structure that stores information specifying
/// a given `amount` (type [`Amount`]) of coins stored at a `token_id`
/// (type [`crate::TokenId`]).
#[cfg_attr(feature = "native", derive(clap::Parser), derive(schemars::JsonSchema))]
#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    Debug,
    Clone,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
)]
pub struct Coins {
    /// The number of tokens
    pub amount: Amount,
    /// The ID of the token
    pub token_id: TokenId,
}

/// The errors that might arise when parsing a `Coins` struct from a string.
#[cfg(feature = "native")]
#[derive(Debug, Error)]
pub enum CoinsFromStrError {
    /// The amount could not be parsed as an u64.
    #[error("Could not parse {input} as a valid amount: {err}")]
    InvalidAmount { input: String, err: ParseIntError },
    /// The input string was malformed, so the `amount` substring could not be extracted.
    #[error("No amount was provided. Make sure that your input is in the format: amount,token_id. Example: 100,sov15vspj48hpttzyvxu8kzq5klhvaczcpyxn6z6k0hwpwtzs4a6wkvqmlyjd6")]
    NoAmountProvided,
    /// The token ID could not be parsed as a valid address.
    #[error("Could not parse {input} as a valid address: {err}")]
    InvalidTokenAddress { input: String, err: anyhow::Error },
    /// The input string was malformed, so the `token_id` substring could not be extracted.
    #[error("No token ID was provided. Make sure that your input is in the format: amount,token_id. Example: 100,sov15vspj48hpttzyvxu8kzq5klhvaczcpyxn6z6k0hwpwtzs4a6wkvqmlyjd6")]
    NoTokenAddressProvided,
}

#[cfg(feature = "native")]
impl FromStr for Coins {
    type Err = CoinsFromStrError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.splitn(2, ',');

        let amount_str = parts.next().ok_or(CoinsFromStrError::NoAmountProvided)?;
        let token_id_str = parts
            .next()
            .ok_or(CoinsFromStrError::NoTokenAddressProvided)?;

        let amount =
            amount_str
                .parse::<Amount>()
                .map_err(|err| CoinsFromStrError::InvalidAmount {
                    input: amount_str.into(),
                    err,
                })?;
        let token_id = TokenId::from_str(token_id_str).map_err(|err| {
            CoinsFromStrError::InvalidTokenAddress {
                input: token_id_str.into(),
                err,
            }
        })?;

        Ok(Self { amount, token_id })
    }
}
impl std::fmt::Display for Coins {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // implement Display for Coins
        write!(f, "token_id={} amount={}", self.token_id, self.amount)
    }
}

/// This struct represents a token in the sov-bank module.
#[derive(borsh::BorshDeserialize, borsh::BorshSerialize, Debug, PartialEq, Clone)]
pub struct Token<S: sov_modules_api::Spec> {
    /// Name of the token.
    pub(crate) name: String,
    /// Total supply of the coins.
    pub(crate) total_supply: u64,
    /// Mapping from user address to user balance.
    pub(crate) balances: sov_modules_api::StateMap<S::Address, Amount>,

    /// Vector containing the authorized minters
    /// Empty vector indicates that the token supply is frozen
    /// Non-empty vector indicates members of the vector can mint.
    /// Freezing a token requires emptying the vector
    /// NOTE: This is explicit, so if a creator doesn't add themselves, then they can't mint
    pub(crate) authorized_minters: Vec<S::Address>,
}

impl<S: sov_modules_api::Spec> Token<S> {
    /// Transfer the amount `amount` of tokens from the address `from` to the address `to`.
    /// First checks that there is enough token of that type stored in `from`. If so, update
    /// the balances of the `from` and `to` accounts.
    pub(crate) fn transfer(
        &self,
        from: &S::Address,
        to: &S::Address,
        amount: Amount,
        working_set: &mut impl StateAccessor,
    ) -> anyhow::Result<()> {
        if from == to {
            return Ok(());
        }
        let from_balance = self
            .check_balance(from, amount, working_set)
            .with_context(|| format!("Incorrect balance on={} for token={}", from, self.name))?;

        // We can't overflow here because the sum must be smaller or eq to `total_supply` which is u64.
        let to_balance = self.balances.get(to, working_set).unwrap_or_default() + amount;

        self.balances.set(from, &from_balance, working_set);
        self.balances.set(to, &to_balance, working_set);
        Ok(())
    }
    /// Burns a specified `amount` of token from the address `from`. First check that the address has enough token to burn,
    /// if not returns an error. Otherwise, update the balances by substracting the amount burnt.
    pub(crate) fn burn(
        &mut self,
        from: &S::Address,
        amount: Amount,
        working_set: &mut WorkingSet<S>,
    ) -> anyhow::Result<()> {
        let new_balance = self.check_balance(from, amount, working_set)?;
        self.balances.set(from, &new_balance, working_set);

        Ok(())
    }

    /// Freezing a token requires emptying the authorized_minter vector
    /// authorized_minter: Vec<Address> is used to determine if the token is frozen or not
    /// If the vector is empty when the function is called, this means the token is already frozen
    pub(crate) fn freeze(&mut self, sender: &S::Address) -> anyhow::Result<()> {
        if self.authorized_minters.is_empty() {
            bail!("Token {} is already frozen", self.name)
        }
        self.is_authorized_minter(sender)?;
        self.authorized_minters = vec![];
        Ok(())
    }

    /// Mints a given `amount` of token sent by `sender` to the specified `mint_to_address`.
    /// Checks that the `authorized_minters` set is not empty for the token and that the `sender`
    /// is an `authorized_minter`. If so, update the balances of token for the `mint_to_address` by
    /// adding the minted tokens. Updates the `total_supply` of that token.
    pub(crate) fn mint(
        &mut self,
        authorizer: &S::Address,
        mint_to_address: &S::Address,
        amount: Amount,
        working_set: &mut WorkingSet<S>,
    ) -> anyhow::Result<()> {
        if self.authorized_minters.is_empty() {
            bail!("Attempt to mint frozen token {}", self.name)
        }

        self.is_authorized_minter(authorizer)?;
        let to_balance: Amount = self
            .balances
            .get(mint_to_address, working_set)
            .unwrap_or_default()
            .checked_add(amount)
            .ok_or(anyhow::Error::msg(
                "Account balance overflow in the mint method of bank module",
            ))?;

        self.balances.set(mint_to_address, &to_balance, working_set);
        self.total_supply = self
            .total_supply
            .checked_add(amount)
            .ok_or(anyhow::Error::msg(
                "Total Supply overflow in the mint method of bank module",
            ))?;
        Ok(())
    }

    fn is_authorized_minter(&self, sender: &S::Address) -> anyhow::Result<()> {
        if !self.authorized_minters.contains(sender) {
            bail!(
                "Sender {} is not an authorized minter of token {}",
                sender,
                self.name
            )
        }
        Ok(())
    }

    // Check that amount can be deducted from address
    // Returns new balance after subtraction.
    fn check_balance(
        &self,
        from: &S::Address,
        amount: Amount,
        working_set: &mut impl StateAccessor,
    ) -> anyhow::Result<Amount> {
        let balance = self.balances.get_or_err(from, working_set)?;
        let new_balance = match balance.checked_sub(amount) {
            Some(from_balance) => from_balance,
            None => bail!("Insufficient funds for {}", from),
        };
        Ok(new_balance)
    }

    /// Creates a token from a given set of parameters.
    /// The `token_name`, `sender` address (as a `u8` slice), and the `salt` (`u64` number) are used as an input
    /// to an hash function that computes the token ID. Then the initial accounts and balances are populated
    /// from the `address_and_balances` slice and the `total_supply` of tokens is updated each time.
    /// Returns a tuple containing the computed `token_id` and the created `token` object.
    pub(crate) fn create(
        token_name: &str,
        address_and_balances: &[(S::Address, u64)],
        authorized_minters: &[S::Address],
        sender: &S::Address,
        salt: u64,
        parent_prefix: &Prefix,
        working_set: &mut WorkingSet<S>,
    ) -> anyhow::Result<(TokenId, Self)> {
        let token_id = super::get_token_id::<S>(token_name, sender, salt);
        let token = Self::create_with_address(
            token_name,
            address_and_balances,
            authorized_minters,
            &token_id,
            parent_prefix,
            working_set,
        )?;
        Ok((token_id, token))
    }

    /// Shouldn't be used directly, only by genesis call
    pub(crate) fn create_with_address(
        token_name: &str,
        address_and_balances: &[(S::Address, u64)],
        authorized_minters: &[S::Address],
        token_id: &TokenId,
        parent_prefix: &Prefix,
        working_set: &mut WorkingSet<S>,
    ) -> anyhow::Result<Token<S>> {
        let token_prefix = prefix_from_address_with_parent(parent_prefix, token_id);
        let balances = sov_modules_api::StateMap::new(token_prefix);

        let mut total_supply: Option<u64> = Some(0);
        for (address, balance) in address_and_balances.iter() {
            balances.set(address, balance, working_set);
            total_supply = total_supply.and_then(|ts| ts.checked_add(*balance));
        }

        let total_supply = match total_supply {
            Some(total_supply) => total_supply,
            None => bail!("Total supply overflow"),
        };

        let mut indices = HashSet::new();
        let mut auth_minter_list = Vec::new();

        for (i, item) in authorized_minters.iter().enumerate() {
            if indices.insert(item.as_ref()) {
                auth_minter_list.push(authorized_minters[i].clone());
            }
        }

        Ok(Token::<S> {
            name: token_name.to_owned(),
            total_supply,
            balances,
            authorized_minters: auth_minter_list,
        })
    }
}
