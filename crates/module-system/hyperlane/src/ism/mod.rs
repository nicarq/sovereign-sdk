//! Implementations of Interchain Security Modules (ISM)

use std::collections::HashSet;

use anyhow::Context as _;
use borsh::{BorshDeserialize, BorshSerialize};
use crypto::compute_hash_for_signatures;
use schemars::JsonSchema;
use secp256k1::ecdsa::RecoverableSignature;
use serde::{Deserialize, Serialize};
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::{Context, GasMeter, GasSpec, HexHash, HexString, SafeVec, Spec};

use crate::types::{keccak256_hash, Message};
use crate::HyperlaneAddress;

type EthAddress = HexString<[u8; 20]>;

const MAX_VALIDATORS: usize = 128;
mod crypto;

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
        validators: SafeVec<EthAddress, MAX_VALIDATORS>,
        /// The number of signatures required to accept a message
        threshold: u32,
    },
}

/// Represents the available ISM types.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
)]
#[repr(u8)]
#[borsh(use_discriminant = false)]
pub enum IsmKind {
    Unused = 0,
    Routing = 1,
    Aggregation = 2,
    LegacyMultisig = 3,
    MerkleRootMultisig = 4,
    MessageIdMultisig = 5,
    Null = 6, // used with relayer carrying no metadata
    CcipRead = 7,
}

/// Metadata for Message ID Multisig ISM
/// See <https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/7656fe1c3865f817d68971ed3c8b939376065283/solidity/contracts/isms/libs/MessageIdMultisigIsmMetadata.sol#L4-L5>
pub struct MessageIdMultisigIsmMetadata {
    pub origin_merkle_tree: HexHash,
    pub merkle_root: HexHash,
    pub merkle_index: u32,
    pub signatures: Vec<RecoverableSignature>,
}

impl Ism {
    /// Verify that a message is valid for the given ISM
    pub fn verify<S: Spec>(
        &self,
        context: &Context<S>,
        message: &Message,
        metadata: &HexString,
        gas_meter: &mut impl GasMeter<Spec = S>,
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
            Ism::MessageIdMultisig {
                validators: original_validators,
                threshold,
            } => {
                let threshold = (*threshold)
                    .try_into()
                    .expect("Threshold is too big for a usize. This is only possible on 16-bit platforms, which are not supported.");
                let metadata = MessageIdMultisigIsmMetadata::decode(&metadata.0, threshold)?;
                let hash = compute_hash_for_signatures(message, &metadata);
                let mut validators: HashSet<&EthAddress> = original_validators.iter().collect();
                // We've already checked that exactly the right number of signatures are present,
                // so we can just iterate over them and check that each one is valid and in the set, removing the validator from the set as we go.
                for signature in metadata.signatures {
                    gas_meter.charge_gas(
                        &<S as GasSpec>::fixed_gas_to_charge_per_signature_verification(),
                    )?;
                    let pubkey_bytes =
                        crypto::ec_recover(hash.0, &signature).context("Invalid signature")?;
                    let addr = HexString(
                        keccak256_hash(pubkey_bytes.0.as_ref()).0[12..]
                            .try_into()
                            .unwrap(),
                    );
                    if !validators.remove(&addr) {
                        // We make the error message more helpful by checking if the validator was in the original list of validators.
                        // This lets us distinguish between double-signing and unknown validators.
                        if original_validators.contains(&addr) {
                            return Err(anyhow::anyhow!(
                                "Not enough unique validators signed the message. 0x{} signed multiple times.", addr
                            ));
                        } else {
                            return Err(anyhow::anyhow!(
                                "Not enough unique validators signed the message. 0x{} is not an allowed validator.", addr
                            ));
                        }
                    }
                }
                Ok(())
            }
        }
    }

    /// Get the ISM type for the given ISM
    pub fn ism_kind(&self) -> IsmKind {
        match self {
            Ism::AlwaysTrust | // AlwaysTrust is used with relayer carrying no metadata
            Ism::TrustedRelayer { .. } => IsmKind::Null, // TrustedRelayer is used with relayer carrying no metadata
            Ism::MessageIdMultisig { .. } => IsmKind::MessageIdMultisig,
        }
    }
}
