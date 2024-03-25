//! Module address definitions

use borsh::{BorshDeserialize, BorshSerialize};
use sov_rollup_interface::{BasicAddress, RollupAddress};

/// Implement type conversions between a `\[u8;32\]` wrapper and a bech32 string representation.
/// This implementation assumes that the wrapper implents a `fn as_bytes(&self) -> &[u8; 32]` as
/// well as `From<\[u8;32\]>` and `AsRef<[u8]>`.
#[macro_export]
macro_rules! impl_bech32_conversion {
    // We make this function generic because the Address type will eventually need a generic
    ($id:ident $( < $generic:ident >)?, $bech32_version:ident, $human_readable_prefix:expr) => {
        /// Implements bech32 display for $id
        #[derive(
            serde::Serialize,
            serde::Deserialize,
            borsh::BorshDeserialize,
            borsh::BorshSerialize,
            Debug,
            PartialEq,
            Clone,
            Eq,
        )]
        #[cfg_attr(
            feature = "arbitrary",
            derive(arbitrary::Arbitrary, proptest_derive::Arbitrary)
        )]
        #[serde(try_from = "String", into = "String")]
        pub struct $bech32_version {
            value: String,
        }

        const __BECH32_HRP: &str = $human_readable_prefix;

        mod __bech32_conversion_impls {
            use super:: $id;
            use super:: $bech32_version;
            use super:: __BECH32_HRP;
            use std::fmt;
            use std::str::FromStr;
            use bech32::{Error, FromBase32, ToBase32};
            /// Converts bytes into a bech32m address, using the provided "Human-Readable Part".
            fn vec_to_bech32m(vec: &[u8], hrp: &str) -> Result<String, Error> {
                let data = vec.to_base32();
                let bech32_addr = bech32::encode(hrp, data, bech32::Variant::Bech32m)?;
                Ok(bech32_addr)
            }

            impl From<$bech32_version> for String {
                fn from(bech: $bech32_version) -> Self {
                    bech.value
                }
            }

            impl core::fmt::Display for $bech32_version {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    write!(f, "{}", self.value)
                }
            }

            /// Converts a bech32m address into bytes, also returning the "Human-Readable Part".
            fn bech32m_to_decoded_vec(bech32_addr: &str) -> Result<(String, Vec<u8>), Error> {
                let (hrp, data, _) = bech32::decode(bech32_addr)?;
                let vec = Vec::<u8>::from_base32(&data)?;
                Ok((hrp, vec))
            }

            impl $(< $generic > )? FromStr for $id $(< $generic > )?{
                type Err = anyhow::Error;

                fn from_str(s: &str) -> Result<Self, Self::Err> {
                    $bech32_version::from_str(s)
                        .map_err(|e| anyhow::anyhow!(e))
                        .map(|item_bech32| item_bech32.into())
                }
            }


            impl FromStr for $bech32_version {
                type Err = $crate::common::Bech32ParseError;

                fn from_str(s: &str) -> Result<Self, $crate::common::Bech32ParseError> {
                    let (hrp, _) = bech32m_to_decoded_vec(s)?;

                    if hrp != __BECH32_HRP {
                        return Err($crate::common::Bech32ParseError::WrongHRP(hrp));
                    }

                    Ok($bech32_version {
                        value: s.to_string(),
                    })
                }
            }

            impl $(< $generic > )? fmt::Display for $id $(< $generic > )? {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    write!(f, "{}", $bech32_version::from(self))
                }
            }

            impl $(< $generic > )? fmt::Debug for $id $(< $generic > )? {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    write!(f, "{:?}", $bech32_version::from(self))
                }
            }

            impl $(< $generic > )? From<$bech32_version> for $id $(< $generic > )? {
                fn from(addr: $bech32_version) -> Self {
                    addr.to_byte_array().into()
                }
            }

            impl $(< $generic > )?  serde::Serialize for $id $(< $generic > )?  {
                fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
                where
                    S: serde::Serializer,
                {
                    if serializer.is_human_readable() {
                        serde::Serialize::serialize(& $bech32_version::from(self), serializer)
                    } else {
                        serde::Serialize::serialize(self.as_bytes(), serializer)
                    }
                }
            }

            impl<'de $(, $generic)?> serde::Deserialize<'de> for $id $(< $generic > )? {
                fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
                where
                    D: serde::Deserializer<'de>,
                {
                    if deserializer.is_human_readable() {
                        let bech: $bech32_version = serde::Deserialize::deserialize(deserializer)?;
                        Ok($id::from(bech.to_byte_array()))
                    } else {
                        let bytes = <[u8; 32] as serde::Deserialize>::deserialize(deserializer)?;
                        Ok(bytes.into())
                    }
                }
            }

            impl $bech32_version {
                pub(crate) fn to_byte_array(&self) -> [u8; 32] {
                    let (_, data) = bech32m_to_decoded_vec(&self.value).unwrap();

                    if data.len() != 32 {
                        panic!("Invalid length {}, should be 32", data.len())
                    }

                    let mut addr_bytes = [0u8; 32];
                    addr_bytes.copy_from_slice(&data);

                    addr_bytes
                }

                /// Returns the human readable prefix for the bech32 representation
                pub fn human_readable_prefix() -> &'static str {
                    __BECH32_HRP
                }
            }


            impl TryFrom<&[u8]> for $bech32_version {
                type Error = bech32::Error;

                fn try_from(addr: &[u8]) -> Result<Self, bech32::Error> {
                    if addr.len() != 32 {
                        return Err(bech32::Error::InvalidLength);
                    }
                    let string = vec_to_bech32m(addr, __BECH32_HRP)?;
                    Ok($bech32_version { value: string })
                }
            }

            impl $(< $generic > )? From<$id $(< $generic > )?> for $bech32_version {
                fn from(addr: $id $(< $generic > )?) -> Self {
                    let string = vec_to_bech32m(addr.as_ref(), __BECH32_HRP).unwrap();
                    $bech32_version { value: string }
                }
            }


            impl $(< $generic > )? From<& $id $(< $generic > )?> for $bech32_version {
                fn from(addr: & $id $(< $generic > )?) -> Self {
                    let string = vec_to_bech32m(addr.as_ref(), __BECH32_HRP).unwrap();
                    $bech32_version { value: string }
                }
            }


            impl TryFrom<String> for $bech32_version {
                type Error = $crate::common::Bech32ParseError;

                fn try_from(addr: String) -> Result<Self, $crate::common::Bech32ParseError> {
                    $bech32_version::from_str(&addr)
                }
            }
        }

    };
}

#[macro_export]
/// Implements a newtype around `\[u8;32\]` which can be displayed in bech32 format with the provided
/// human readable prefix.
macro_rules! impl_hash32_type {
    ($id:ident, $bech32_version:ident, $human_readable_prefix:expr) => {
        #[derive(
            Clone, Copy, PartialEq, Eq, Hash, borsh::BorshDeserialize, borsh::BorshSerialize,
        )]
        #[cfg_attr(feature = "native", derive(schemars::JsonSchema))]
        /// A globally unique identifier.
        pub struct $id([u8; 32]);

        impl From<[u8; 32]> for $id {
            fn from(inner: [u8; 32]) -> Self {
                Self(inner)
            }
        }

        impl AsRef<[u8]> for $id {
            fn as_ref(&self) -> &[u8] {
                &self.0
            }
        }

        impl $id {
            /// Exposes the inner bytes of $id
            pub const fn as_bytes(&self) -> &[u8; 32] {
                &self.0
            }

            /// Converts the id to a bech32 string
            pub fn to_bech32(&self) -> $bech32_version {
                self.into()
            }

            /// Returns the human readable prefix for the bech32 representation
            pub fn bech32_prefix() -> &'static str {
                $human_readable_prefix
            }
        }

        $crate::impl_bech32_conversion!($id, $bech32_version, $human_readable_prefix);
    };
}

impl_bech32_conversion!(Address, AddressBech32, ADDRESS_PREFIX);

/// Module address representation
#[cfg_attr(all(feature = "native", feature = "std"), derive(schemars::JsonSchema))]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[derive(PartialEq, Hash, Clone, Copy, Eq, BorshDeserialize, BorshSerialize)]

pub struct Address {
    addr: [u8; 32],
}

impl AsRef<[u8]> for Address {
    fn as_ref(&self) -> &[u8] {
        &self.addr
    }
}

impl Address {
    /// Creates a new address containing the given bytes
    pub const fn new(addr: [u8; 32]) -> Self {
        Self { addr }
    }

    /// Exposes the inner bytes of the Address
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.addr
    }
}

impl<'a> TryFrom<&'a [u8]> for Address {
    type Error = anyhow::Error;

    fn try_from(addr: &'a [u8]) -> Result<Self, Self::Error> {
        if addr.len() != 32 {
            anyhow::bail!("Address must be 32 bytes long");
        }
        let mut addr_bytes = [0u8; 32];
        addr_bytes.copy_from_slice(addr);
        Ok(Self { addr: addr_bytes })
    }
}

impl From<[u8; 32]> for Address {
    fn from(addr: [u8; 32]) -> Self {
        Self { addr }
    }
}

impl BasicAddress for Address {}
impl RollupAddress for Address {}

// TODO(@preston-evans98): unify core and modules, then
// enable sov-modules-macros and do this
// #[sov_modules_macros::config_constant]
const ADDRESS_PREFIX: &str = "sov";

#[cfg(test)]
mod test {

    use super::*;

    #[test]
    fn test_address_serialization() {
        let address = Address::from([11; 32]);
        let data: String = serde_json::to_string(&address).unwrap();
        let deserialized_address = serde_json::from_str::<Address>(&data).unwrap();

        assert_eq!(address, deserialized_address);
        assert_eq!(
            deserialized_address.to_string(),
            "sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9stup8tx"
        );
    }
}
