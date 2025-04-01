//! Implementations of Interchain Security Modules (ISM)

use borsh::{BorshDeserialize, BorshSerialize};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::{Context, HexHash, HexString, Spec, TxState};

use crate::types::Message;
use crate::HyperlaneAddress;

type EthAddress = HexString<[u8; 20]>;

/// Represents the available ISMs
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
    JsonSchema,
    UniversalWallet,
)]
pub enum Ism {
    /// Performs no validation. Will accept any message - useful for testing
    AlwaysTrust,
    /// Accepts all messages from a trusted relayer
    TrustedRelayer {
        /// The address of the trusted relayer, in [`HyperlaneAddress`] format
        relayer: HexHash,
    },
    /// Accepts messages if signed by `threshold` or more of the provided `validators`
    MessageIdMultisig {
        /// The addresses of the validators
        validators: Vec<EthAddress>,
        /// The number of signatures required to accept a message
        threshold: u32,
    },
}

impl Ism {
    /// Verify that a message is valid for the given ISM
    pub fn verify<S: Spec>(
        &self,
        context: &Context<S>,
        _message: &Message,
        _metadata: &HexString,
        _state: &mut impl TxState<S>,
    ) -> anyhow::Result<()>
    where
        S::Address: HyperlaneAddress,
    {
        match self {
            Ism::AlwaysTrust => Ok(()),
            Ism::TrustedRelayer { relayer } => {
                anyhow::ensure!(
                    &context.sender().to_sender() == relayer,
                    "Only {} is trusted to relay messages for this ISM",
                    relayer,
                );
                Ok(())
            }
            Ism::MessageIdMultisig { .. } => {
                anyhow::bail!("MessageIdMultisig is not yet implemented");
            }
        }
    }
}
