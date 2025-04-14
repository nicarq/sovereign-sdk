//! Cryptographic primitves / helpers used in hyperlane

use anyhow::{ensure, Context, Result};
use secp256k1::ecdsa::{RecoverableSignature, RecoveryId};
use secp256k1::PublicKey;
use sha3::{Digest, Keccak256};
use sov_modules_api::{HexHash, HexString};

use crate::types::{EthAddress, Message, StorageLocation};

/// The EIP-191 prefix for an Ethereum signed message
/// <https://eips.ethereum.org/EIPS/eip-191>
pub const ETH_SIGNED_MESSAGE_PREFIX: &str = "\x19Ethereum Signed Message:\n";

/// Computes the hash of the message that was used to construct the validators' signatures.
/// Note that this is *not* the simple keccak256 hash of the `Message` struct. A reference implementation can be found here:
/// <https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/7656fe1c3865f817d68971ed3c8b939376065283/solidity/contracts/libs/CheckpointLib.sol#L28>
pub fn compute_hash_for_signatures(
    message: &Message,
    origin_merkle_tree: &HexHash,
    merkle_root: &HexHash,
    merkle_index: u32,
) -> EthSignHash {
    // https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/7656fe1c3865f817d68971ed3c8b939376065283/solidity/contracts/libs/CheckpointLib.sol#L80
    let domain_hash = DomainHash::new(
        message.origin_domain,
        &origin_merkle_tree.0,
        HashKind::Hyperlane,
    );
    // https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/7656fe1c3865f817d68971ed3c8b939376065283/solidity/contracts/libs/CheckpointLib.sol#L37
    let multisig_hash = MultisigHash::new(domain_hash, *merkle_root, merkle_index, message.id());
    EthSignHash::new(multisig_hash.0)
}

/// Decodes [`RecoverableSignature`] out of a slice of bytes.
// See <https://github.com/eigerco/hyperlane-monorepo/blob/b68fe264b3585ecd9d95a5ec2ec2d7defbe907d2/rust/sealevel/libraries/ecdsa-signature/src/lib.rs#L40>
pub fn decode_signature(bytes: &[u8]) -> Result<RecoverableSignature> {
    ensure!(
        bytes.len() == 65,
        "Compact recoverable signature must be 65 bytes"
    );

    let mut recovery_id = bytes[64];
    if recovery_id == 27 || recovery_id == 28 {
        recovery_id -= 27;
    }
    if recovery_id > 1 {
        Err(secp256k1::Error::InvalidRecoveryId)?;
    }
    Ok(RecoverableSignature::from_compact(
        &bytes[..64],
        RecoveryId::from_i32(recovery_id.into())?,
    )?)
}

/// A 64 byte uncompressed ECDSA public key.
pub struct EcdsaPubKeyBytes(pub [u8; 64]);

impl From<PublicKey> for EcdsaPubKeyBytes {
    fn from(value: PublicKey) -> Self {
        let pubkey_bytes: [u8; 65] = value.serialize_uncompressed();
        // The first byte is the compression flag, which we don't care about.
        // https://stackoverflow.com/questions/66383584/how-to-extract-uncompressed-public-key-from-secp256k1
        EcdsaPubKeyBytes(
            pubkey_bytes[1..]
                .try_into()
                .expect("Uncompressed public key has 64 bytes after the first byte"),
        )
    }
}

/// Recover public key from message hash and signature
pub fn ec_recover(
    digest: impl Into<[u8; 32]>,
    signature: &RecoverableSignature,
) -> Result<EcdsaPubKeyBytes> {
    let message = secp256k1::Message::from_digest(digest.into());
    let public_key = signature.recover(&message).context("Invalid signature")?;
    Ok(public_key.into())
}

/// Derive ethereum address from public key
pub fn eth_address_from_public_key(pub_key: impl Into<EcdsaPubKeyBytes>) -> EthAddress {
    let pub_key = pub_key.into();
    let hash = keccak256_hash(&pub_key.0);
    // truncate first 12 bytes
    HexString(hash.0[12..].try_into().expect("Must be exactly 20 bytes"))
}

/// Convert a slice of bytes into a 32-byte hash using the keccak256 algorithm.
#[must_use]
pub fn keccak256_hash(bz: &[u8]) -> HexHash {
    use sha3::{Digest, Keccak256};

    Keccak256::digest(bz).into()
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
    /// Create a new domain hash
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
    /// Create a new multisig hash
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
    /// Create a new eth signature hash
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

/// A hash used in signing messages for the validator announcements
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AnnouncementHash(pub [u8; 32]);

impl AnnouncementHash {
    /// Create a new validator announcement hash
    pub fn new(domain_hash: DomainHash, location: &StorageLocation) -> Self {
        Self(
            Keccak256::new()
                .chain_update(domain_hash.0)
                .chain_update(location)
                .finalize()
                .into(),
        )
    }
}
