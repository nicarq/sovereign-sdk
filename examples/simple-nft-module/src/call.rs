use anyhow::{bail, Result};
#[cfg(feature = "native")]
use sov_modules_api::macros::CliWalletArg;
use sov_modules_api::{CallResponse, Context, EventEmitter, Spec, StateAccessor, TxState};

use crate::{Event, NonFungibleToken};

#[cfg_attr(
    feature = "native",
    derive(serde::Serialize),
    derive(serde::Deserialize),
    derive(CliWalletArg),
    derive(schemars::JsonSchema),
    schemars(bound = "S::Address: ::schemars::JsonSchema", rename = "CallMessage")
)]
#[derive(borsh::BorshDeserialize, borsh::BorshSerialize, Debug, PartialEq, Clone)]
/// A transaction handled by the NFT module. Mints, Transfers, or Burns an NFT by id
pub enum CallMessage<S: Spec> {
    /// Mint a new token
    Mint {
        /// The id of new token. Caller is an owner
        id: u64,
    },
    /// Transfer existing token to the new owner
    Transfer {
        /// The address to which the token will be transferred.
        to: S::Address,
        /// The token id to transfer
        id: u64,
    },
    /// Burn existing token
    Burn {
        /// The token id to burn
        id: u64,
    },
}

impl<S: Spec> NonFungibleToken<S> {
    pub(crate) fn mint(
        &self,
        id: u64,
        context: &Context<S>,
        working_set: &mut impl TxState<S>,
    ) -> Result<CallResponse> {
        if self.owners.get(&id, working_set).is_some() {
            bail!("Token with id {} already exists", id);
        }

        self.give_nft(context.sender(), id, working_set)?;
        self.emit_event(working_set, "simple_nft_mint", Event::Mint { id });

        Ok(CallResponse::default())
    }

    pub(crate) fn transfer(
        &self,
        id: u64,
        to: S::Address,
        context: &Context<S>,
        working_set: &mut impl TxState<S>,
    ) -> Result<CallResponse> {
        let Some(token_owner) = self.owners.get(&id, working_set) else {
            bail!("Token with id {} does not exist", id);
        };
        if &token_owner != context.sender() {
            bail!("Only token owner can transfer token");
        }

        self.remove_nft(id, working_set)?;
        self.give_nft(&to, id, working_set)?;
        self.emit_event(working_set, "nft_transfer", Event::Transfer { id });

        Ok(CallResponse::default())
    }

    pub(crate) fn burn(
        &self,
        id: u64,
        context: &Context<S>,
        working_set: &mut impl TxState<S>,
    ) -> Result<CallResponse> {
        let Some(token_owner) = self.owners.get(&id, working_set) else {
            bail!("Token with id {} does not exist", id);
        };
        if &token_owner != context.sender() {
            bail!("Only token owner can burn token");
        }

        self.remove_nft(id, working_set)?;
        self.emit_event(working_set, "nft_burned", Event::Burn { id });

        Ok(CallResponse::default())
    }

    pub(crate) fn give_nft(
        &self,
        owner: &S::Address,
        nft_id: u64,
        working_set: &mut impl StateAccessor,
    ) -> anyhow::Result<()> {
        self.owners.set(&nft_id, owner, working_set);
        Ok(())
    }

    fn remove_nft(&self, nft_id: u64, working_set: &mut impl StateAccessor) -> anyhow::Result<()> {
        self.owners.remove(&nft_id, working_set);
        Ok(())
    }
}
