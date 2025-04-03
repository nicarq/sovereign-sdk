use anyhow::Context;
use secp256k1::ecdsa::{RecoverableSignature, RecoveryId};
use sha3::{Digest, Keccak256};
use sov_modules_api::{HexHash, HexString};

use super::MessageIdMultisigIsmMetadata;
use crate::types::Message;

/// The EIP-191 prefix for an Ethereum signed message
/// <https://eips.ethereum.org/EIPS/eip-191>
const ETH_SIGNED_MESSAGE_PREFIX: &str = "\x19Ethereum Signed Message:\n";

/// Computes the hash of the message that was used to construct the validators' signatures.
/// Note that this is *not* the simple keccak256 hash of the `Message` struct. A reference implementation can be found here:
/// <https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/7656fe1c3865f817d68971ed3c8b939376065283/solidity/contracts/libs/CheckpointLib.sol#L28>
pub fn compute_hash_for_signatures(
    message: &Message,
    metadata: &MessageIdMultisigIsmMetadata,
) -> EthSignHash {
    // https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/7656fe1c3865f817d68971ed3c8b939376065283/solidity/contracts/libs/CheckpointLib.sol#L80
    let domain_hash = DomainHash::new(
        message.origin_domain,
        &metadata.origin_merkle_tree.0,
        HashKind::Hyperlane,
    );
    // https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/7656fe1c3865f817d68971ed3c8b939376065283/solidity/contracts/libs/CheckpointLib.sol#L37
    let multisig_hash = MultisigHash::new(
        domain_hash,
        metadata.merkle_root,
        metadata.merkle_index,
        message.id(),
    );
    EthSignHash::new(multisig_hash.0)
}

/// A 64 byte uncompressed ECDSA public key.
pub struct EcdsaPubKeyBytes(pub [u8; 64]);

pub fn ec_recover(
    digest: impl Into<[u8; 32]>,
    signature: &RecoverableSignature,
) -> anyhow::Result<EcdsaPubKeyBytes> {
    let message = secp256k1::Message::from_digest(digest.into());
    let signature = signature.recover(&message).context("Invalid signature")?;
    let pubkey_bytes: [u8; 65] = signature.serialize_uncompressed();
    // The first byte is the compression flag, which we don't care about.
    // https://stackoverflow.com/questions/66383584/how-to-extract-uncompressed-public-key-from-secp256k1
    Ok(EcdsaPubKeyBytes(pubkey_bytes[1..].try_into().expect(
        "Uncompressed public key has 64 bytes after the first byte",
    )))
}

impl MessageIdMultisigIsmMetadata {
    /// Decode the metadata from a message.
    // See <https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/7656fe1c3865f817d68971ed3c8b939376065283/solidity/contracts/isms/libs/MessageIdMultisigIsmMetadata.sol#L9>
    // for the reference on the expected format.
    pub fn decode(metadata: &[u8], num_signatures: usize) -> anyhow::Result<Self> {
        let expected_len = 32 + 32 + 4 + num_signatures * 65;
        anyhow::ensure!(
            metadata.len() == expected_len,
            "Invalid metadata length: expected {}, got {}",
            expected_len,
            metadata.len()
        );
        // Safety: we've already checked the length so all unwraps are infallible
        let origin_merkle_tree: HexHash = HexString(metadata[0..32].try_into().unwrap());
        let merkle_root: HexHash = HexString(metadata[32..64].try_into().unwrap());
        let merkle_index = u32::from_be_bytes(metadata[64..68].try_into().unwrap());
        let signatures = metadata[68..]
            .chunks_exact(65)
            .map(|chunk| {
                // <https://github.com/eigerco/hyperlane-monorepo/blob/b68fe264b3585ecd9d95a5ec2ec2d7defbe907d2/rust/sealevel/libraries/ecdsa-signature/src/lib.rs#L40>
                let mut recovery_id = chunk[64];
                if recovery_id == 27 || recovery_id == 28 {
                    recovery_id -= 27;
                }
                if recovery_id > 1 {
                    return Err(secp256k1::Error::InvalidRecoveryId);
                }
                RecoverableSignature::from_compact(
                    &chunk[..64],
                    RecoveryId::from_i32(recovery_id.into())?,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Sanity check that the number of signatures is correct. We already checked the length at the top of this function,
        // so a failure here indicates a bug.
        assert!(
            signatures.len() == num_signatures,
            "Invalid number of signatures: expected {}, got {}",
            num_signatures,
            signatures.len()
        );
        Ok(Self {
            origin_merkle_tree,
            merkle_root,
            merkle_index,
            signatures,
        })
    }
}

/// Domain separators for Hyperlane signed messages
pub enum HashKind {
    #[allow(dead_code)]
    /// The message is a validator "announcement"
    // Note: This will be used once we re-enable validator announcements
    HyperlaneAnnouncement,
    /// The message is a Hyperlane protocol message
    Hyperlane,
}

impl AsRef<[u8]> for HashKind {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::HyperlaneAnnouncement => "HYPERLANE_ANNOUNCEMENT".as_bytes(),
            Self::Hyperlane => "HYPERLANE".as_bytes(),
        }
    }
}

/// A domain hash is a hash of the domain ID, mailbox address, and one of set of well known domain separators
// See <https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/7656fe1c3865f817d68971ed3c8b939376065283/solidity/contracts/libs/CheckpointLib.sol#L80>
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DomainHash(pub [u8; 32]);

impl DomainHash {
    pub fn new(domain: u32, mailbox_addr: &[u8; 32], kind: HashKind) -> Self {
        Self(
            Keccak256::new()
                .chain_update(domain.to_be_bytes())
                .chain_update(mailbox_addr)
                .chain_update(kind)
                .finalize()
                .into(),
        )
    }
}

/// A hash used in signing messages for the `MultiSig` ISM
// https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/7656fe1c3865f817d68971ed3c8b939376065283/solidity/contracts/libs/CheckpointLib.sol#L37
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MultisigHash(pub [u8; 32]);
impl MultisigHash {
    #[must_use]
    pub fn new(
        domain_hash: DomainHash,
        merkle_root: HexHash,
        index: u32,
        message_id: HexHash,
    ) -> Self {
        Self(
            Keccak256::new()
                .chain_update(domain_hash.0)
                .chain_update(merkle_root)
                .chain_update(index.to_be_bytes())
                .chain_update(message_id)
                .finalize()
                .into(),
        )
    }
}

/// An ethereum-style hash - using a well known prefix, a length, and some data.
/// See <https://eips.ethereum.org/EIPS/eip-191>
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct EthSignHash(pub [u8; 32]);
impl EthSignHash {
    pub fn new(value: impl AsRef<[u8]>) -> Self {
        Self(
            Keccak256::new()
                .chain_update(ETH_SIGNED_MESSAGE_PREFIX)
                .chain_update(value.as_ref().len().to_string())
                .chain_update(value.as_ref())
                .finalize()
                .into(),
        )
    }
}
