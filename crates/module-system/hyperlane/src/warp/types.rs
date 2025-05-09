use borsh::{BorshDeserialize, BorshSerialize};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sov_bank::TokenId;
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::{Amount, HexHash, Spec};

use crate::Ism;

/// Represents the source of the token in Hyperlane
#[derive(
    BorshDeserialize,
    BorshSerialize,
    Serialize,
    Deserialize,
    Debug,
    PartialEq,
    Clone,
    UniversalWallet,
    Eq,
    JsonSchema,
    Hash,
    strum::EnumDiscriminants,
)]
#[strum_discriminants(derive(BorshSerialize, BorshDeserialize, strum::AsRefStr))]
#[strum_discriminants(name(TokenKindId))]
pub enum TokenKind {
    /// The token is natively issued on some remote chain, so the local representation is a synthetic token.
    // https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/9334f3345724886953bdc980b2dff717b80bb87c/solidity/contracts/token/HypERC20.sol#L17
    Synthetic {
        /// The ID of the remote token.
        remote_token_id: HexHash,
        /// The number of decimal places for the local (synthetic) token.
        synthetic_decimals: Option<u8>,
        /// How many remote tokens are equivalent to one local token.
        /// <https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/9334f3345724886953bdc980b2dff717b80bb87c/solidity/contracts/token/libs/FungibleTokenRouter.sol#L17-L29>
        ///
        /// We multiply the token amount sent to the Warp route by this scale when sending a message.
        /// and divide by it when issuing tokens after receiving a message.
        synthetic_scale: Option<Amount>,
    },
    /// The token is natively issued on the local chain.
    Collateral {
        /// The ID of the token on the local chain.
        token: TokenId,
    },
    /// The token is the native token of the local chain.
    Native,
}

/// Represents the source of the token in Hyperlane
#[derive(
    BorshDeserialize,
    BorshSerialize,
    Serialize,
    Deserialize,
    Debug,
    PartialEq,
    Clone,
    UniversalWallet,
    Eq,
    JsonSchema,
    Hash,
)]
pub enum StoredTokenKind {
    /// The token is natively issued on some remote chain, so the local representation is a synthetic token.
    // https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/9334f3345724886953bdc980b2dff717b80bb87c/solidity/contracts/token/HypERC20.sol#L17
    Synthetic {
        /// The ID of the remote token.
        remote_token_id: HexHash,
        /// The number of decimal places for the local (synthetic) token.
        synthetic_decimals: Option<u8>,
        /// How many remote tokens are equivalent to one local token.
        /// <https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/9334f3345724886953bdc980b2dff717b80bb87c/solidity/contracts/token/libs/FungibleTokenRouter.sol#L17-L29>
        ///
        /// We multiply the token amount sent to the Warp route by this scale when sending a message.
        /// and divide by it when issuing tokens after receiving a message.
        synthetic_scale: Option<Amount>,
        /// The token ID of the token on the *local* chain.
        local_token_id: TokenId,
    },
    /// The token is natively issued on the local chain.
    Collateral {
        /// The ID of the token on the local chain.
        token: TokenId,
    },
    /// The token is the native token of the local chain.
    Native,
}

#[derive(
    BorshDeserialize,
    BorshSerialize,
    Serialize,
    Deserialize,
    Debug,
    PartialEq,
    Clone,
    UniversalWallet,
    Eq,
    JsonSchema,
    Hash,
)]
/// The address of a remote router.
pub struct RemoteRouterAddress(pub HexHash);

impl std::fmt::Display for RemoteRouterAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for RemoteRouterAddress {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(HexHash::from_str(s)?))
    }
}

/// Multiplies a u128 by a u128 and returns a big-endian u256
fn mul_u128s(scale: u128, amount: u128) -> [u8; 32] {
    type U256 = ruint::Uint<256, 4>;

    let out = U256::try_from(scale).unwrap() * U256::try_from(amount).unwrap();
    out.to_be_bytes()
}

fn div_u256(amount: [u8; 32], scale: u128) -> anyhow::Result<Amount> {
    type U256 = ruint::Uint<256, 4>;

    let out = U256::from_be_bytes(amount) / U256::try_from(scale).unwrap();
    Ok(Amount(out.try_into().map_err(|_| {
        anyhow::anyhow!("Amount may not exceed 2^128 - 1 after scaling")
    })?))
}

impl StoredTokenKind {
    /// Scales the amount to account for the differences in decimals when sending to a destination chain.
    /// <https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/9334f3345724886953bdc980b2dff717b80bb87c/solidity/contracts/token/libs/FungibleTokenRouter.sol#L17-L29>
    pub fn outbound_amount(&self, local_amount: Amount) -> [u8; 32] {
        match self {
            StoredTokenKind::Synthetic {
                synthetic_scale: Some(synthetic_scale),
                ..
            } => mul_u128s(synthetic_scale.0, local_amount.0),
            StoredTokenKind::Synthetic {
                synthetic_scale: None,
                ..
            }
            | StoredTokenKind::Collateral { .. }
            | StoredTokenKind::Native => {
                let mut out = [0u8; 32];
                out[16..].copy_from_slice(&local_amount.0.to_be_bytes());
                out
            }
        }
    }

    /// Scales the amount to account for the differences in decimals when receiving a message.
    /// <https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/9334f3345724886953bdc980b2dff717b80bb87c/solidity/contracts/token/libs/FungibleTokenRouter.sol#L17-L29>
    pub fn inbound_amount(&self, amount: [u8; 32]) -> anyhow::Result<Amount> {
        match self {
            StoredTokenKind::Synthetic {
                synthetic_scale: Some(synthetic_scale),
                ..
            } => div_u256(amount, synthetic_scale.0),
            StoredTokenKind::Synthetic {
                synthetic_scale: None,
                ..
            }
            | StoredTokenKind::Collateral { .. }
            | StoredTokenKind::Native => {
                anyhow::ensure!(
                    amount.starts_with(&[0u8; 16]),
                    "Amount may not exceed 2^128 - 1"
                );
                let mut out = [0u8; 16];
                out.copy_from_slice(&amount[16..]);
                Ok(Amount(u128::from_be_bytes(out)))
            }
        }
    }
}

impl TokenKind {
    /// Returns the token ID and kind for the token kind.
    pub fn id_and_kind(&self) -> (HexHash, TokenKindId) {
        match self {
            TokenKind::Synthetic {
                remote_token_id, ..
            } => (*remote_token_id, TokenKindId::Synthetic),
            TokenKind::Collateral { token, .. } => {
                ((*token.as_bytes()).into(), TokenKindId::Collateral)
            }
            TokenKind::Native => (
                (*sov_bank::config_gas_token_id().as_bytes()).into(),
                TokenKindId::Native,
            ),
        }
    }
}

pub(crate) type WarpRouteId = HexHash;

/// The authority that can modify the route.
#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    Serialize,
    Deserialize,
    Debug,
    PartialEq,
    Clone,
    JsonSchema,
    UniversalWallet,
    Eq,
)]
pub enum Admin<S: Spec> {
    /// No admin - the route is immutable.
    None,
    /// Allow the specified address to modify the route. This is extremely insecure,
    /// but it seems to be common practice in Hyperlane.
    InsecureOwner(S::Address),
}

/// Represents the warp route instance.
#[derive(
    borsh::BorshDeserialize, borsh::BorshSerialize, Serialize, Deserialize, Debug, PartialEq, Clone,
)]
pub struct WarpRouteInstance<S: Spec> {
    /// The source of the token.
    pub token_source: StoredTokenKind,
    /// The authority that can modify the route, if any.
    pub admin: Admin<S>,
    /// The ISM for this route.
    pub ism: Ism,
}

#[derive(
    BorshDeserialize,
    BorshSerialize,
    Serialize,
    Deserialize,
    Debug,
    PartialEq,
    Clone,
    Eq,
    JsonSchema,
)]
/// A key for a router, consisting of route ID and destination domain.
pub struct RouterKey {
    /// The address of the router.
    pub route_id: WarpRouteId,
    /// The domain of the router.
    pub remote_domain: u32,
}

impl std::fmt::Display for RouterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.route_id, self.remote_domain)
    }
}

impl std::str::FromStr for RouterKey {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split(':');
        let route_id = parts
            .next()
            .ok_or(anyhow::anyhow!(
                "Invalid router key: missing separator token `:`"
            ))?
            .parse()?;
        let destination_domain = parts
            .next()
            .ok_or(anyhow::anyhow!(
                "Invalid router key: missing destination domain"
            ))?
            .parse()?;
        anyhow::ensure!(
            parts.next().is_none(),
            "Invalid router key: Too many separator tokens (`:`)"
        );
        Ok(RouterKey {
            route_id,
            remote_domain: destination_domain,
        })
    }
}
