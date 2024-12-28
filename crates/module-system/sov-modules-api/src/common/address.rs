//! Module id definitions

#[cfg(feature = "arbitrary")]
use arbitrary::{Arbitrary, Unstructured};
use borsh::{BorshDeserialize, BorshSerialize};
use derivative::Derivative;
use sha2::digest::typenum::U32;
use sha2::Digest;
use sov_modules_macros::config_value_private;
use sov_rollup_interface::common::HexString;
use sov_rollup_interface::crypto::PublicKey;
use sov_rollup_interface::BasicAddress;

/// Implement type conversions between a `\[u8;$len\]` wrapper and a bech32 string representation.
/// This implementation assumes that the wrapper implents a `fn as_bytes(&self) -> &[u8;$len]` as
/// well as `From<\[u8;$len\]>` and `AsRef<[u8]>`.
#[macro_export]
macro_rules! impl_bech32_conversion {
    // We make this function generic because the Address type will eventually need a generic
    ($id:ident $( < $generic:ident >)?, $bech32_version:ident, $human_readable_prefix:expr, $len:expr) => {
        /// A pre-validated bech32 representation of $id
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
        #[serde(try_from = "String", into = "String")]
        pub struct $bech32_version (
            /// A validated bech32 string
            String,
        );

        const fn __bech32_hrp() -> &'static str {
            $human_readable_prefix
        }

        mod __bech32_conversion_impls {
            use super:: $id;
            use super:: $bech32_version;
            use super:: __bech32_hrp;
            use std::fmt;
            use std::str::FromStr;
            use $crate::prelude::{bech32, serde, anyhow};
            use bech32::primitives::decode::{UncheckedHrpstring, CheckedHrpstring};
            use bech32::{Bech32m, Hrp};

            impl From<$bech32_version> for String {
                fn from(bech: $bech32_version) -> Self {
                    bech.0
                }
            }

            impl core::fmt::Display for $bech32_version {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    write!(f, "{}", self.0)
                }
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
                    let hrp_string = CheckedHrpstring::new::<Bech32m>(s)?;

                    if hrp_string.hrp().as_str() != __bech32_hrp() {
                        return Err($crate::common::Bech32ParseError::WrongHRP(hrp_string.hrp().to_string()));
                    }

                    // In Bech32, the "data part" is the portion after the HRP (human-readable prefix) and
                    // separator ('1'), but before the final 6-character checksum. Each Bech32 data character
                    // represents 5 bits. Thus, to convert from the length of this data part (in 5-bit
                    // characters) back to bytes, we multiply that length by 5 and then divide by 8. If the
                    // result is not exactly $len, the address would decode to a different number of bytes
                    // than expected.
                    //
                    // Reference: https://github.com/bitcoin/bips/blob/master/bip-0173.mediawiki
                    let data_len = hrp_string.data_part_ascii_no_checksum().len();
                    let data_bytes = (data_len * 5 as usize) / 8;
                    if data_bytes != ($len as usize) {
                        return Err($crate::common::Bech32ParseError::WrongLength($len, data_bytes));
                    }

                    Ok($bech32_version (
                        s.to_string(),
                    ))
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

            impl $(< $generic > )? serde::Serialize for $id $(< $generic > )?  {
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
                        let bytes = <[u8; $len] as serde::Deserialize>::deserialize(deserializer)?;
                        Ok(bytes.into())
                    }
                }
            }

            impl $bech32_version {
                pub(crate) fn to_byte_array(&self) -> [u8; $len] {
                    let hrp_string = UncheckedHrpstring::new(&self.0)
                        .expect("Bech32 was validated at construction")
                        .remove_checksum::<Bech32m>();

                    let mut addr_bytes = [0u8; $len];
                    for (l, r) in addr_bytes.iter_mut().zip(hrp_string.byte_iter()) {
                        *l = r;
                    }
                    addr_bytes
                }

                /// Returns the human readable prefix for the bech32 representation
                pub fn human_readable_prefix() -> &'static str {
                    __bech32_hrp()
                }
            }

            impl $(< $generic > )? From<$id $(< $generic > )?> for $bech32_version {
                fn from(addr: $id $(< $generic > )?) -> Self {
                    let string = bech32::encode::<Bech32m>(Hrp::parse_unchecked(__bech32_hrp()), addr.as_ref()).expect("Encoding to string is infallible");
                    $bech32_version(string)
                }
            }


            impl $(< $generic > )? From<& $id $(< $generic > )?> for $bech32_version {
                fn from(addr: & $id $(< $generic > )?) -> Self {
                    let string = bech32::encode::<Bech32m>(Hrp::parse_unchecked(__bech32_hrp()), addr.as_ref()).expect("Encoding to string is infallible");
                    $bech32_version(string)
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
/// Implements a newtype around `\[u8;$len\]` which can be displayed in bech32 format with the provided
/// human readable prefix.
macro_rules! impl_hash32_type {
    ($id:ident, $bech32_version:ident, $human_readable_prefix:expr) => {
        $crate::impl_hash32_type!($id, $bech32_version, $human_readable_prefix, 32);
    };

    ($id:ident, $bech32_version:ident, $human_readable_prefix:expr, $len:expr) => {
        #[derive(
            Clone,
            Copy,
            PartialEq,
            Eq,
            Hash,
            borsh::BorshDeserialize,
            borsh::BorshSerialize,
            schemars::JsonSchema,
            sov_modules_api::macros::UniversalWallet,
        )]
        #[cfg_attr(
            feature = "arbitrary",
            derive(
                sov_modules_api::prelude::arbitrary::Arbitrary,
                sov_modules_api::prelude::proptest_derive::Arbitrary
            )
        )]
        /// A globally unique identifier.
        pub struct $id(
            #[sov_wallet(display(bech32m(prefix = "__impl_hash32_type_prefix()")))] [u8; $len],
        );

        const fn __impl_hash32_type_prefix() -> &'static str {
            $human_readable_prefix
        }

        impl From<[u8; $len]> for $id {
            fn from(inner: [u8; $len]) -> Self {
                Self(inner)
            }
        }

        impl AsRef<[u8]> for $id {
            fn as_ref(&self) -> &[u8] {
                &self.0
            }
        }

        impl<'a> TryFrom<&'a [u8]> for $id {
            type Error = anyhow::Error;

            fn try_from(id: &'a [u8]) -> Result<Self, Self::Error> {
                if id.len() != $len {
                    anyhow::bail!("Id must be {} bytes long", $len);
                }
                let mut id_bytes = [0u8; $len];
                id_bytes.copy_from_slice(id);
                Ok(Self(id_bytes))
            }
        }

        impl $id {
            /// Exposes the inner bytes of $id
            pub const fn as_bytes(&self) -> &[u8; $len] {
                &self.0
            }

            /// Converts the id to a bech32 string
            pub fn to_bech32(&self) -> $bech32_version {
                self.into()
            }

            /// Returns the human readable prefix for the bech32 representation
            pub const fn bech32_prefix() -> &'static str {
                __impl_hash32_type_prefix()
            }

            /// Creates a new $id containing the given bytes. This function is needed in addition
            /// to the `From` trait to allow for const conversion
            pub const fn from_const_slice(addr: [u8; $len]) -> Self {
                Self(addr)
            }
        }

        $crate::impl_bech32_conversion!($id, $bech32_version, $human_readable_prefix, $len);
    };
}

impl_bech32_conversion!(Address<H>, AddressBech32, address_prefix(), 28);

/// Module ID representation.
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
#[derive(Derivative, BorshDeserialize, BorshSerialize)]
#[derivative(Copy, Hash, PartialEq, Eq, Ord)]
pub struct Address<H> {
    addr: [u8; 28],
    #[derivative(Hash = "ignore", PartialEq = "ignore", Ord = "ignore")]
    phantom: std::marker::PhantomData<H>,
}

// Deriving this seems to trigger
// <https://rust-lang.github.io/rust-clippy/master/index.html#/non_canonical_partial_ord_impl>
// because of `derivative`, so let's implement it manually.
impl<H> PartialOrd for Address<H> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// Serialize Address without field labels. This changes the output from `{ addr: sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9stup8tx}`
// to just `sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9stup8tx`
impl<H: 'static> sov_rollup_interface::sov_universal_wallet::schema::OverrideSchema for Address<H> {
    type Output = AddressSchema;
}

/// Address prefix
pub const fn address_prefix() -> &'static str {
    config_value_private!("ADDRESS_PREFIX")
}

#[derive(sov_rollup_interface::sov_universal_wallet::UniversalWallet)]
#[allow(dead_code)]
#[doc(hidden)]
pub struct AddressSchema(#[sov_wallet(display(bech32m(prefix = "address_prefix()")))] [u8; 28]);

// We manually implement clone so that we can silence this clippy warning.
// Derivative has o facility to enable that.
#[allow(clippy::non_canonical_clone_impl)]
impl<H> Clone for Address<H> {
    fn clone(&self) -> Self {
        Self {
            addr: self.addr,
            phantom: std::marker::PhantomData,
        }
    }
}

impl<H> schemars::JsonSchema for Address<H> {
    fn schema_name() -> String {
        "Address".to_string()
    }

    fn json_schema(_gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        serde_json::from_value(serde_json::json!({
            "type": "string",
            // TODO(@neysofu): this regex pattern is currently correct, but it
            // must be updated if `Address` allows for custom prefixes, instead
            // of hardcoding `sov`.
            "pattern": "^sov1[a-zA-Z0-9]+$",
            "description": "Address",
        }))
        .unwrap()
    }
}

impl<H: Digest<OutputSize = U32>, T: PublicKey> From<&T> for Address<H> {
    fn from(value: &T) -> Self {
        value.credential_id::<H>().0.into()
    }
}

impl<H> AsRef<[u8]> for Address<H> {
    fn as_ref(&self) -> &[u8] {
        &self.addr
    }
}

impl<H> Address<H> {
    /// Creates a new address containing the given bytes
    pub const fn new(addr: [u8; 28]) -> Self {
        Self {
            addr,
            phantom: std::marker::PhantomData,
        }
    }

    /// Exposes the inner bytes of the Address
    pub const fn as_bytes(&self) -> &[u8; 28] {
        &self.addr
    }
}

impl<'a, H> TryFrom<&'a [u8]> for Address<H> {
    type Error = anyhow::Error;

    fn try_from(addr: &'a [u8]) -> Result<Self, Self::Error> {
        if addr.len() != 28 {
            anyhow::bail!("Address must be 28 bytes long");
        }
        let mut addr_bytes = [0u8; 28];
        addr_bytes.copy_from_slice(addr);
        Ok(Self {
            addr: addr_bytes,
            phantom: std::marker::PhantomData,
        })
    }
}

impl<H> From<[u8; 28]> for Address<H> {
    fn from(addr: [u8; 28]) -> Self {
        Self {
            addr,
            phantom: std::marker::PhantomData,
        }
    }
}

impl<H> From<HexString<[u8; 32]>> for Address<H> {
    fn from(value: HexString<[u8; 32]>) -> Self {
        Self::try_from(&value.0.as_slice()[0..28]).unwrap()
    }
}

impl<H> Address<H> {
    /// Creates a new $id containing the given bytes. This function is needed in addition
    /// to the `From` trait to allow for const conversions
    pub const fn from_const_slice(addr: [u8; 28]) -> Self {
        Self {
            addr,
            phantom: std::marker::PhantomData,
        }
    }
}

// Implement arbitrary by hand because the derive can't handle PhantomData
#[cfg(feature = "arbitrary")]
impl<'a, H> Arbitrary<'a> for Address<H> {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let addr = u.arbitrary()?;
        Ok(Self {
            addr,
            phantom: std::marker::PhantomData,
        })
    }
}

impl<H: Send + Sync + 'static> BasicAddress for Address<H> {}

#[cfg(test)]
mod test {

    use core::str::FromStr;

    use sha2::Sha256;
    use sov_risc0_adapter::crypto::Risc0PublicKey;
    use sov_rollup_interface::crypto::PublicKeyHex;
    use sov_universal_wallet::schema::Schema;

    use super::*;

    #[test]
    fn test_address_serialization() {
        let address = Address::from([11; 28]);
        let data: String = serde_json::to_string(&address).unwrap();
        let deserialized_address = serde_json::from_str::<Address<Sha256>>(&data).unwrap();

        assert_eq!(address, deserialized_address);
        assert_eq!(
            deserialized_address.to_string(),
            "sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skqm7ehv"
        );
    }

    #[test]
    fn test_address_schema() {
        let address: Address<Sha256> = Address::from([11; 28]);
        let schema = Schema::of_single_type::<Address<Sha256>>();
        assert_eq!(
            schema
                .display(0, &borsh::to_vec(&address).unwrap())
                .unwrap(),
            "sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skqm7ehv"
        );
    }

    #[test]
    /// Enforces that we reject the original (less secure) `bech32` encoding for our address type.
    /// Our addresses should use bech32m only.
    fn test_rejects_non_m_bech32_variant() {
        assert!(Address::<Sha256>::from_str(
            "sov11zy3rx3z4vemc3xgq42aueh0wlugjyv6y24n80zyeqz4tkp42j22"
        )
        .is_err());
    }

    #[test]
    fn test_rejects_wrong_length() {
        assert!(Address::<Sha256>::from_str(
            "sov10ay4dyaukwpqnteh2h32l6rfurecsmzu5sl78aj7qzc0g2vvnwesa0k6"
        )
        .is_err());
    }

    #[test]
    fn test_address_conversion() {
        let pub_key_hex: PublicKeyHex =
            "022e229198d957bf0c0a504e7d7bcec99a1d62cccc7861ed2452676ad0323ad8"
                .try_into()
                .unwrap();

        let pub_key = Risc0PublicKey::try_from(&pub_key_hex).unwrap();

        let sov_address = pub_key.to_address::<Address<Sha256>>();

        let expected_addr =
            Address::<Sha256>::from_str("sov10ay4dyaukwpqnteh2h32l6rfurecsmzu5sl78aj7qzc0g286a0l")
                .unwrap();

        assert_eq!(sov_address, expected_addr);
    }
}

#[cfg(all(test, feature = "arbitrary"))]
mod arbitrary_tests {
    use proptest::prelude::any;
    use proptest::proptest;
    use sha2::Sha256;
    use sov_test_utils::validate_schema;

    use super::*;

    proptest! {
        #[test]
        fn json_schema_is_valid(item in any::<Address<Sha256>>()) {
            validate_schema(&item).unwrap();
        }
    }
}
