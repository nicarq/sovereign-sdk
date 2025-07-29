use borsh::{BorshDeserialize, BorshSerialize};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use sov_rollup_interface::zk::CryptoSpec;

use crate::evm::public_key::EthereumPublicKey;
use crate::evm::signature::EthereumSignature;

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    BorshDeserialize,
    BorshSerialize,
)]
/// A [`CryptoSpec`] implementation for EVM rollups.
/// Uses the secp256k1 signature scheme with keccak256 hashes for signatures,
/// and sha256 as the default hasher for other operations.
pub struct EvmCryptoSpec;

impl CryptoSpec for EvmCryptoSpec {
    #[cfg(feature = "native")]
    type PrivateKey = crate::evm::private_key::EthereumPrivateKey;

    type PublicKey = EthereumPublicKey;

    type Hasher = Sha256;

    type Signature = EthereumSignature;
}
