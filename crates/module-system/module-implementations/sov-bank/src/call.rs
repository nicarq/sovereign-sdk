use anyhow::{bail, Context as _, Result};
use schemars::JsonSchema;
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::{
    Context, EventEmitter, SafeString, SafeVec, Spec, StateAccessor, StateReader, TxState,
};
use sov_state::User;
use strum::{EnumDiscriminants, EnumIs, EnumIter, VariantArray};

use crate::event::Event;
use crate::utils::{Payable, TokenHolderRef};
use crate::{Amount, Bank, Coins, Token, TokenId};

/// The maximum number of addresses that can be authorized to mint or freeze a token.
pub const MAX_ADMINS: usize = 20;

/// This enumeration represents the available call messages for interacting with the sov-bank module.
#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
    Debug,
    PartialEq,
    Eq,
    Clone,
    JsonSchema,
    EnumDiscriminants,
    EnumIs,
    UniversalWallet,
)]
#[schemars(bound = "S::Address: ::schemars::JsonSchema", rename = "CallMessage")]
#[serde(rename_all = "snake_case")]
#[strum_discriminants(derive(VariantArray, EnumIs, EnumIter))]
pub enum CallMessage<S: Spec> {
    /// Creates a new token with the specified name and initial balance.
    CreateToken {
        /// The name of the new token.
        token_name: SafeString,
        /// The initial balance of the new token.
        initial_balance: Amount,
        /// The address of the account that the new tokens are minted to.
        mint_to_address: S::Address,
        /// Admins list.
        admins: SafeVec<S::Address, MAX_ADMINS>,
    },

    /// Transfers a specified amount of tokens to the specified address.
    #[sov_wallet(show_as = "Transfer to address {} {}.")]
    #[sov_wallet(template("transfer"))]
    Transfer {
        /// The address to which the tokens will be transferred.
        #[sov_wallet(template("transfer" = input("to")))]
        to: S::Address,
        /// The amount of tokens to transfer.
        coins: Coins,
    },

    /// Burns a specified amount of tokens.
    Burn {
        /// The amount of tokens to burn.
        coins: Coins,
    },

    /// Mints a specified amount of tokens.
    Mint {
        /// The amount of tokens to mint.
        coins: Coins,
        /// Address to mint tokens to
        mint_to_address: S::Address,
    },

    /// Freezes a token so that the supply is frozen
    Freeze {
        /// Address of the token to be frozen
        token_id: TokenId,
    },
}

impl<S: Spec> Bank<S> {
    /// Creates a token from a set of configuration parameters.
    /// Checks if a token already exists at that address. If so return an error.
    #[allow(clippy::too_many_arguments)]
    pub fn create_token(
        &self,
        token_name: String,
        initial_balance: Amount,
        mint_to_address: impl Payable<S>,
        admins: Vec<impl Payable<S>>,
        minter: impl Payable<S>,
        state: &mut impl TxState<S>,
    ) -> Result<TokenId> {
        tracing::debug!(%minter, "Create token request");

        let mint_to_address = mint_to_address.as_token_holder();
        let admins = admins
            .iter()
            .map(|minter| minter.as_token_holder())
            .collect::<Vec<_>>();

        let (token_id, token) = Token::<S>::create(
            &token_name,
            &[(mint_to_address, initial_balance)],
            &admins,
            &minter,
            self.tokens.prefix(),
            state,
        )?;

        if self.tokens.get(&token_id, state)?.is_some() {
            bail!(
                "Token with id already exists {}, name={} minter={}",
                token_id,
                token_name,
                minter.as_token_holder()
            );
        }

        self.tokens.set(&token_id, &token, state)?;

        tracing::info!(
            %token_id,
            %token_name,
            %minter,
            %initial_balance,
            %mint_to_address,
            ?admins,
            "Token created"
        );

        self.emit_event(
            state,
            Event::TokenCreated {
                token_name: token_name.clone(),
                coins: Coins {
                    amount: initial_balance,
                    token_id,
                },
                mint_to_address: mint_to_address.into(),
                minter: minter.as_token_holder().into(),
                admins: admins.iter().map(|m| m.into()).collect(),
            },
        );
        Ok(token_id)
    }

    /// Transfers the set of `coins` to the address specified by `to`.
    pub fn transfer(
        &self,
        to: impl Payable<S>,
        coins: Coins,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        tracing::debug!("Transfer token request");

        let to = to.as_token_holder();
        let sender = context.sender();

        self.transfer_from(sender, to, coins.clone(), state)?;

        tracing::info!(
            from = %sender,
            %to,
            %coins,
            "Token transfer successful"
        );

        self.emit_event(
            state,
            Event::TokenTransferred {
                from: sender.as_token_holder().into(),
                to: to.into(),
                coins,
            },
        );
        Ok(())
    }

    /// Burns (permanently destroys) the specified amount of tokens, removing them from circulation.
    /// This operation cannot be undone - burned tokens are permanently lost.
    ///
    /// # Errors
    ///
    /// If the specified token ID does not exist.
    ///
    /// If the `owner` has insufficient token balance to burn the requested amount.
    /// No tokens will be burned in this case.
    ///
    /// If the requested burn amount exceeds the token's total supply.
    /// No tokens will be burned in this case.
    pub fn burn(
        &self,
        coins: Coins,
        owner: impl Payable<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        tracing::debug!("Handling Burn call");

        let mut token = self
            .tokens
            .get_or_err(&coins.token_id, state)?
            .with_context(|| format!("Failed to get token_id={}", &coins.token_id))?;

        let owner = owner.as_token_holder();
        token.burn(owner, coins.amount, state).with_context(|| {
            format!(
                "Failed to burn token_id={} owner={}",
                &coins.token_id, owner
            )
        })?;
        self.tokens.set(&coins.token_id, &token, state)?;

        tracing::info!(
            id = %coins.token_id,
            name = token.name,
            burnt_amount = coins.amount,
            %owner,
            updated_total_supply = token.total_supply,
            "Successfully burnt tokens"
        );

        self.emit_event(
            state,
            Event::TokenBurned {
                owner: owner.into(),
                coins,
            },
        );

        Ok(())
    }

    /// Burns coins from an externally owned address ("EOA")
    pub(crate) fn burn_from_eoa(
        &self,
        coins: Coins,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        self.burn(coins, context.sender(), state)?;
        Ok(())
    }

    /// Mints the `coins`to the address `mint_to_identity` using the externally owned account ("EOA") supplied by
    /// `context.sender()` as the authorizer.
    /// Returns an error if the token ID doesn't exist or `context.sender()` is not authorized to mint tokens.
    ///
    /// On success, it updates the `self.tokens` set to store the new balance.
    pub fn mint_from_eoa(
        &self,
        coins: Coins,
        mint_to_identity: impl Payable<S>,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        self.mint(
            coins,
            mint_to_identity,
            TokenHolderRef::from(&context.sender()),
            state,
        )
    }

    /// Mints the `coins` to the  `mint_to_identity` if `authorizer` is an allowed minter.
    /// Returns an error if the token ID doesn't exist or `context.sender()` is not authorized to mint tokens.
    ///
    /// On success, it updates the `self.tokens` set to store the new minted address.
    pub fn mint(
        &self,
        coins: Coins,
        mint_to_identity: impl Payable<S>,
        authorizer: impl Payable<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        tracing::debug!(%authorizer, "Mint token request");

        let mint_to_identity = mint_to_identity.as_token_holder();
        let mut token = self
            .tokens
            .get_or_err(&coins.token_id, state)?
            .with_context(|| format!("Failed to get token_id={}", &coins.token_id))?;

        let authorizer = authorizer.as_token_holder();
        token
            .mint(authorizer, mint_to_identity, coins.amount, state)
            .with_context(|| format!("Failed to mint token_id={}", &coins.token_id))?;
        self.tokens.set(&coins.token_id, &token, state)?;

        tracing::info!(
            %authorizer,
            token_id = %coins.token_id,
            amount = coins.amount,
            minted_to = %mint_to_identity,
            "Successfully minted tokens"
        );

        self.emit_event(
            state,
            Event::TokenMinted {
                mint_to_identity: mint_to_identity.into(),
                authorizer: authorizer.into(),
                coins: coins.clone(),
            },
        );

        Ok(())
    }

    /// Tries to freeze the token ID `token_id`.
    /// Returns an error if the token ID doesn't exist,
    /// otherwise calls the [`Token::freeze`] function, and update the token set upon success.
    pub(crate) fn freeze(
        &self,
        token_id: TokenId,
        context: &Context<S>,
        state: &mut impl TxState<S>,
    ) -> Result<()> {
        let sender_ref = context.sender();
        let sender = sender_ref.as_token_holder();

        tracing::debug!(freezer = %sender, "Freeze token request");

        let mut token = self
            .tokens
            .get_or_err(&token_id, state)?
            .with_context(|| format!("Failed to get token_id={}", &token_id))?;

        token
            .freeze(sender)
            .with_context(|| format!("Failed to freeze token_id={}", &token_id))?;

        self.tokens.set(&token_id, &token, state)?;

        tracing::info!(
            freezer = %sender,
            %token_id,
            "Successfully froze tokens"
        );

        self.emit_event(
            state,
            Event::TokenFrozen {
                freezer: sender.into(),
                token_id,
            },
        );

        Ok(())
    }
}

impl<S: Spec> Bank<S> {
    /// Transfers the set of `coins` from the address `from` to the address `to`.
    ///
    /// Returns an error if the token ID doesn't exist.
    pub fn transfer_from(
        &self,
        from: impl Payable<S>,
        to: impl Payable<S>,
        coins: Coins,
        state: &mut impl StateAccessor,
    ) -> Result<()> {
        let from = from.as_token_holder();
        let to = to.as_token_holder();
        let token = self
            .tokens
            .get_or_err(&coins.token_id, state)?
            .with_context(|| format!("Failed to get token_id={}", &coins.token_id))?;

        token
            .transfer(from, to, coins.amount, state)
            .with_context(|| format!("Failed to transfer token_id={}", &coins.token_id))?;

        Ok(())
    }

    /// Retrieve a token by the provided token id.
    pub fn get_token<Accessor: StateAccessor>(
        &self,
        token_id: &TokenId,
        state: &mut Accessor,
    ) -> Result<Option<Token<S>>, <Accessor as StateReader<User>>::Error> {
        self.tokens.get(token_id, state)
    }

    /// Helper function to return the balance of the token stored at `token_id`
    /// for the user having the address `user_address` from the underlying storage. If the token ID doesn't exist, or
    /// if the user doesn't have tokens of that type, return `None`. Otherwise, wrap the resulting balance in `Some`.
    pub fn get_balance_of<Accessor: StateAccessor>(
        &self,
        user_address: impl Payable<S>,
        token_id: TokenId,
        state: &mut Accessor,
    ) -> Result<Option<u64>, <Accessor as StateReader<User>>::Error> {
        let user_address = user_address.as_token_holder();
        self.tokens
            .get(&token_id, state)?
            .and_then(|token| token.balances.get(&user_address, state).transpose())
            .transpose()
    }

    /// Get the name of a token by ID
    pub fn get_token_name<Accessor: StateReader<User>>(
        &self,
        token_id: &TokenId,
        state: &mut Accessor,
    ) -> Result<Option<String>, Accessor::Error> {
        let token = self.tokens.get(token_id, state)?;
        Ok(token.map(|token| token.name))
    }

    /// Returns the total supply of the token with the given `token_id`.
    pub fn get_total_supply_of<Accessor: StateAccessor>(
        &self,
        token_id: &TokenId,
        state: &mut Accessor,
    ) -> Result<Option<u64>, <Accessor as StateReader<User>>::Error> {
        Ok(self
            .tokens
            .get(token_id, state)?
            .map(|token| token.total_supply))
    }
}
