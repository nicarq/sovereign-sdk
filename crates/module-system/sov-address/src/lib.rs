#[cfg(feature = "evm")]
mod ethereum_address;
#[cfg(feature = "evm")]
mod multi_address_evm;

use std::str::FromStr;

use borsh::{BorshDeserialize, BorshSerialize};
#[cfg(feature = "evm")]
pub use ethereum_address::EthereumAddress;
#[cfg(feature = "evm")]
pub use multi_address_evm::MultiAddressEvm;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::Sha256;
use sov_modules_api::macros::UniversalWallet;
use sov_modules_api::Address;

/// A generic VM-compatible address enum, enabling supporting for both Sov SDK standard SHA-256 derived addresses and VM-specific addresses.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    JsonSchema,
    UniversalWallet,
)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[sov_wallet(hide_tag)]
pub enum MultiAddress<VmAddress> {
    /// A standard address derived from a SHA-256 hash of a public key.
    Standard(Address<Sha256>),
    /// A VM-specific address type, allowing native support of the VM's address format.
    Vm(VmAddress),
}

impl<VmAddress> From<sov_modules_api::AddressBech32> for MultiAddress<VmAddress> {
    fn from(value: sov_modules_api::AddressBech32) -> Self {
        Self::Standard(value.into())
    }
}

impl<VmAddress> From<Address<Sha256>> for MultiAddress<VmAddress> {
    fn from(value: Address<Sha256>) -> Self {
        Self::Standard(value)
    }
}

impl<'a, K: sov_rollup_interface::crypto::PublicKey, VmAddress> From<&'a K>
    for MultiAddress<VmAddress>
where
    Address<Sha256>: From<&'a K>,
{
    fn from(value: &'a K) -> Self {
        Self::Standard(Address::<Sha256>::from(value))
    }
}

impl<VmAddress: std::fmt::Display> std::fmt::Display for MultiAddress<VmAddress> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MultiAddress::Standard(addr) => addr.fmt(f),
            MultiAddress::Vm(addr) => std::fmt::Display::fmt(&addr, f),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) enum DeSerHelper<VmAddress> {
    Standard(Address<Sha256>),
    Vm(VmAddress),
}

impl<VmAddress: Serialize + std::fmt::Display> Serialize for MultiAddress<VmAddress> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if serializer.is_human_readable() {
            // For the human readable format, we rely on `Display` impl of the `VmAddress` to
            // be distinct from the `Standard` address, which should be prefixed with something like `sov`.
            serializer.serialize_str(&self.to_string())
        } else {
            let helper = match self {
                Self::Standard(addr) => DeSerHelper::Standard(*addr),
                Self::Vm(addr) => DeSerHelper::Vm(addr),
            };
            helper.serialize(serializer)
        }
    }
}

impl<'de, VmAddress> Deserialize<'de> for MultiAddress<VmAddress>
where
    VmAddress: Deserialize<'de>,
    MultiAddress<VmAddress>: std::str::FromStr,
    <MultiAddress<VmAddress> as std::str::FromStr>::Err: std::fmt::Display,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            let s: String = Deserialize::deserialize(deserializer)?;
            MultiAddress::<VmAddress>::from_str(&s).map_err(serde::de::Error::custom)
        } else {
            let helper = DeSerHelper::deserialize(deserializer)?;
            match helper {
                DeSerHelper::Standard(addr) => Ok(MultiAddress::Standard(addr)),
                DeSerHelper::Vm(addr) => Ok(MultiAddress::Vm(addr)),
            }
        }
    }
}
