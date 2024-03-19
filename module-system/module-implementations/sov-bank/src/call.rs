use anyhow::{bail, Context as _, Result};
#[cfg(feature = "native")]
use sov_modules_api::macros::CliWalletArg;
use sov_modules_api::{CallResponse, Context, EventEmitter, StateAccessor, WorkingSet};

use crate::event::Event;
use crate::{Amount, Bank, Coins, Token};

/// This enumeration represents the available call messages for interacting with the sov-bank module.
#[cfg_attr(
    feature = "native",
    derive(CliWalletArg),
    derive(schemars::JsonSchema),
    schemars(bound = "S::Address: ::schemars::JsonSchema", rename = "CallMessage")
)]
#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
    Debug,
    PartialEq,
    Clone,
)]
pub enum CallMessage<S: sov_modules_api::Spec> {
    /// Creates a new token with the specified name and initial balance.
    CreateToken {
        /// Random value use to create a unique token address.
        salt: u64,
        /// The name of the new token.
        token_name: String,
        /// The initial balance of the new token.
        initial_balance: Amount,
        /// The address of the account that the new tokens are minted to.
        minter_address: S::Address,
        /// Authorized minter list.
        authorized_minters: Vec<S::Address>,
    },

    /// Transfers a specified amount of tokens to the specified address.
    Transfer {
        /// The address to which the tokens will be transferred.
        to: S::Address,
        /// The amount of tokens to transfer.
        coins: Coins<S>,
    },

    /// Burns a specified amount of tokens.
    Burn {
        /// The amount of tokens to burn.
        coins: Coins<S>,
    },

    /// Mints a specified amount of tokens.
    Mint {
        /// The amount of tokens to mint.
        coins: Coins<S>,
        /// Address to mint tokens to
        minter_address: S::Address,
    },

    /// Freezes a token so that the supply is frozen
    Freeze {
        /// Address of the token to be frozen
        token_address: S::Address,
    },
}

impl<S: sov_modules_api::Spec> Bank<S> {
    /// Creates a token from a set of configuration parameters.
    /// Checks if a token already exists at that address. If so return an error.
    #[allow(clippy::too_many_arguments)]
    pub fn create_token(
        &self,
        token_name: String,
        salt: u64,
        initial_balance: Amount,
        minter_address: S::Address,
        authorized_minters: Vec<S::Address>,
        context: &Context<S>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<S::Address> {
        tracing::info!(%token_name, %salt, %initial_balance, %minter_address, sender= %context.sender(), "Create token request");
        let (token_address, token) = Token::<S>::create(
            &token_name,
            &[(minter_address, initial_balance)],
            &authorized_minters,
            context.sender(),
            salt,
            self.tokens.prefix(),
            working_set,
        )?;

        if self.tokens.get(&token_address, working_set).is_some() {
            bail!(
                "Token {} at {} address already exists",
                token_name,
                token_address
            );
        }

        self.tokens.set(&token_address, &token, working_set);
        self.emit_event(
            working_set,
            "token_created",
            Event::<S>::TokenCreated {
                token_address: token_address.clone(),
            },
        );
        tracing::info!(%token_name, %token_address, "Token created");
        Ok(token_address)
    }

    /// Transfers the set of `coins` to the address specified by `to`.
    pub fn transfer(
        &self,
        to: S::Address,
        coins: Coins<S>,
        context: &Context<S>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<CallResponse> {
        self.transfer_from(context.sender(), &to, coins.clone(), working_set)
            .map(|response| {
                // TODO: move this back into the body of transfer_from once we create a trait for StateAccessor + EventEmitter
                // https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/168
                self.emit_event(
                    working_set,
                    "token_transfer",
                    Event::TokenTransferred {
                        token_address: coins.token_address,
                        amount: coins.amount,
                    },
                );
                response
            })
    }

    /// Burns the set of `coins`.
    ///
    /// If there is no token at the address specified in the
    /// [`Coins`] structure, return an error; on success it updates the total
    /// supply of tokens.
    pub fn burn(
        &self,
        coins: Coins<S>,
        owner: &S::Address,
        working_set: &mut WorkingSet<S>,
    ) -> Result<()> {
        let context_logger = || format!("Failed to burn coins({}) from owner {}", coins, owner,);
        let mut token = self
            .tokens
            .get_or_err(&coins.token_address, working_set)
            .with_context(context_logger)?;
        token
            .burn(owner, coins.amount, working_set)
            .with_context(context_logger)?;
        token.total_supply -= coins.amount;
        self.tokens.set(&coins.token_address, &token, working_set);

        Ok(())
    }

    /// Burns coins from an externally owned address ("EOA")
    pub(crate) fn burn_from_eoa(
        &self,
        coins: Coins<S>,
        context: &Context<S>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<CallResponse> {
        self.burn(coins, context.sender(), working_set)?;
        Ok(CallResponse::default())
    }

    /// Mints the `coins`to the address `mint_to_address` using the externally owned account ("EOA") supplied by
    /// `context.sender()` as the authorizer.
    /// Returns an error if the token address doesn't exist or `context.sender()` is not authorized to mint tokens.
    ///
    /// On success, it updates the `self.tokens` set to store the new balance.
    pub fn mint_from_eoa(
        &self,
        coins: &Coins<S>,
        mint_to_address: &S::Address,
        context: &Context<S>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<()> {
        self.mint(coins, mint_to_address, context.sender(), working_set)
    }

    /// Mints the `coins` to the address `mint_to_address` if `authorizer` is an allowed minter.
    /// Returns an error if the token address doesn't exist or `context.sender()` is not authorized to mint tokens.
    ///
    /// On success, it updates the `self.tokens` set to store the new minted address.
    pub fn mint(
        &self,
        coins: &Coins<S>,
        mint_to_address: &S::Address,
        authorizer: &S::Address,
        working_set: &mut WorkingSet<S>,
    ) -> Result<()> {
        let context_logger = || {
            format!(
                "Failed mint coins({}) to {} by authorizer {}",
                coins, mint_to_address, authorizer
            )
        };
        let mut token = self
            .tokens
            .get_or_err(&coins.token_address, working_set)
            .with_context(context_logger)?;
        token
            .mint(authorizer, mint_to_address, coins.amount, working_set)
            .with_context(context_logger)?;
        self.tokens.set(&coins.token_address, &token, working_set);

        Ok(())
    }

    /// Tries to freeze the token address `token_address`.
    /// Returns an error if the token address doesn't exist,
    /// otherwise calls the [`Token::freeze`] function, and update the token set upon success.
    pub(crate) fn freeze(
        &self,
        token_address: S::Address,
        context: &Context<S>,
        working_set: &mut WorkingSet<S>,
    ) -> Result<CallResponse> {
        let context_logger = || {
            format!(
                "Failed freeze token_address={} by sender {}",
                token_address,
                context.sender()
            )
        };
        let mut token = self
            .tokens
            .get_or_err(&token_address, working_set)
            .with_context(context_logger)?;
        token
            .freeze(context.sender())
            .with_context(context_logger)?;
        self.tokens.set(&token_address, &token, working_set);

        Ok(CallResponse::default())
    }
}

impl<S: sov_modules_api::Spec> Bank<S> {
    /// Transfers the set of `coins` from the address `from` to the address `to`.
    ///
    /// Returns an error if the token address doesn't exist.
    pub fn transfer_from(
        &self,
        from: &S::Address,
        to: &S::Address,
        coins: Coins<S>,
        working_set: &mut impl StateAccessor,
    ) -> Result<CallResponse> {
        let context_logger = || {
            format!(
                "Failed transfer from={} to={} of coins({})",
                from, to, coins
            )
        };
        let token = self
            .tokens
            .get_or_err(&coins.token_address, working_set)
            .with_context(context_logger)?;
        token
            .transfer(from, to, coins.amount, working_set)
            .with_context(context_logger)?;
        Ok(CallResponse::default())
    }

    /// Helper function used by the rpc method [`balance_of`](Bank::balance_of) to return the balance of the token stored at `token_address`
    /// for the user having the address `user_address` from the underlying storage. If the token address doesn't exist, or
    /// if the user doesn't have tokens of that type, return `None`. Otherwise, wrap the resulting balance in `Some`.
    pub fn get_balance_of(
        &self,
        user_address: S::Address,
        token_address: S::Address,
        working_set: &mut impl StateAccessor,
    ) -> Option<u64> {
        self.tokens
            .get(&token_address, working_set)
            .and_then(|token| token.balances.get(&user_address, working_set))
    }

    /// Get the name of a token by address
    pub fn get_token_name(
        &self,
        token_address: &S::Address,
        working_set: &mut WorkingSet<S>,
    ) -> Option<String> {
        let token = self.tokens.get(token_address, working_set);
        token.map(|token| token.name)
    }

    /// Total supply of a token stored at the address `token_address`
    pub fn get_total_supply_of(
        &self,
        token_address: &S::Address,
        working_set: &mut WorkingSet<S>,
    ) -> Option<u64> {
        self.tokens
            .get(token_address, working_set)
            .map(|token| token.total_supply)
    }
}

/// Creates a new prefix from an already existing prefix `parent_prefix` and a `token_address`
/// by extending the parent prefix.
pub(crate) fn prefix_from_address_with_parent<S: sov_modules_api::Spec>(
    parent_prefix: &sov_state::Prefix,
    token_address: &S::Address,
) -> sov_state::Prefix {
    let mut prefix = parent_prefix.as_aligned_vec().clone().into_inner();
    prefix.extend_from_slice(format!("{}", token_address).as_bytes());
    sov_state::Prefix::new(prefix)
}
