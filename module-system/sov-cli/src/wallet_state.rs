use std::path::{Path, PathBuf};
use std::{fs, mem};

use borsh::{BorshDeserialize, BorshSerialize};
use semver::Version;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sov_modules_api::transaction::{Transaction, UnsignedTransaction};
use sov_modules_api::{clap, CryptoSpec, PrivateKey, Spec};

/// A struct representing the current state of the CLI wallet
#[derive(Debug, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + DeserializeOwned, Tx: Serialize + DeserializeOwned")]
pub struct WalletState<Tx, S: sov_modules_api::Spec>
where
    Tx: BorshSerialize + BorshDeserialize,
{
    /// The accumulated transactions to be submitted to the DA layer
    pub unsent_transactions: Vec<UnsignedTransaction<S, Tx>>,
    /// The addresses in the wallet
    pub addresses: AddressList<S>,
    /// The addresses in the wallet
    pub rpc_url: Option<String>,
    /// The version of the library that serialized the state.
    pub version: String,
}

impl<Tx, S> Default for WalletState<Tx, S>
where
    Tx: Serialize + DeserializeOwned + BorshSerialize + BorshDeserialize,
    S: Spec,
{
    fn default() -> Self {
        Self {
            unsent_transactions: Vec::new(),
            addresses: AddressList {
                addresses: Vec::new(),
            },
            rpc_url: None,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

impl<Tx, S> WalletState<Tx, S>
where
    Tx: Serialize + DeserializeOwned + BorshSerialize + BorshDeserialize,
    S: Spec,
{
    /// Load the wallet state from the given path on disk
    pub fn load(path: impl AsRef<Path>) -> Result<Self, anyhow::Error> {
        let path = path.as_ref();
        if path.exists() {
            let data = fs::read(path)?;

            let version = env!("CARGO_PKG_VERSION")
                .parse::<Version>()
                .expect("Failed to parse the library version");

            let value: Value = serde_json::from_slice(data.as_slice()).map_err(|e|
                anyhow::anyhow!(
                    "Failed to read the JSON state of the wallet. Check if `{}` points to a valid JSON state file. Error: {e}",
                    path.display()
                )
            )?;

            let data_version = value
                .get("version")
                .and_then(Value::as_str)
                .and_then(|v| v.parse::<Version>().ok())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Failed to read the version from state of the wallet. Check if `{}` points to a valid JSON state file.",
                        path.display()
                    )
                })?;

            if version.major != data_version.major
                || version.major == 0 && version.minor != data_version.minor
            {
                anyhow::bail!(
                    "The version that created the state on the state file `{}` is `{data_version}`, and the library is `{version}`.

This discrepancy may result in data layout inconsistency. Consider one of the following options:

- Migrate the wallet state to the current version. Check the repository documentation.
- Use a different data directory via the environment variable `SOV_WALLET_DIR_ENV_VAR` to generate an empty state with the current version.
- Delete the state file` so the wallet will generate a new empty state.
- Manually update the version on the state file to `{version}`. Warning: this approach assumes the data layout to be the same.",
                    path.display(),
                );
            }

            let state = serde_json::from_slice(data.as_slice())?;

            Ok(state)
        } else {
            Ok(Default::default())
        }
    }

    /// Save the wallet state to the given path on disk
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), anyhow::Error> {
        let data = serde_json::to_string_pretty(self)?;
        fs::write(path, data)?;
        Ok(())
    }

    /// Returns the serialized, signed transactions of the state.
    ///
    /// Consumes unsigned transactions, signing them with the provided key and using the supplied
    /// nonce for each transaction, incrementally.
    pub fn take_signed_transactions(
        &mut self,
        signing_key: &<S::CryptoSpec as CryptoSpec>::PrivateKey,
        nonce: u64,
    ) -> Vec<Vec<u8>> {
        mem::take(&mut self.unsent_transactions)
            .into_iter()
            .enumerate()
            .map(
                |(
                    offset,
                    UnsignedTransaction {
                        tx,
                        chain_id,
                        max_priority_fee,
                        max_fee,
                        gas_limit,
                    },
                )| {
                    let runtime_msg = tx.try_to_vec().unwrap();
                    let tx = Transaction::<S>::new_signed_tx(
                        signing_key,
                        runtime_msg,
                        chain_id,
                        max_priority_fee,
                        max_fee,
                        gas_limit,
                        nonce.wrapping_add(offset as u64),
                    );

                    tx.try_to_vec().unwrap()
                },
            )
            .collect()
    }
}

/// A struct representing private key and associated address
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + DeserializeOwned")]
pub struct PrivateKeyAndAddress<S: sov_modules_api::Spec> {
    /// Private key of the address
    pub private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey,
    /// Address associated from the private key
    pub address: S::Address,
}

impl<S: sov_modules_api::Spec> PrivateKeyAndAddress<S> {
    /// Returns boolean if the private key matches default address
    pub fn is_matching_to_default(&self) -> bool {
        self.private_key.to_address::<S::Address>() == self.address
    }

    /// Randomly generates a new private key and address
    pub fn generate() -> Self {
        let private_key = <S::CryptoSpec as CryptoSpec>::PrivateKey::generate();
        let address = private_key.to_address::<S::Address>();
        Self {
            private_key,
            address,
        }
    }

    /// Generates valid private key and address from given private key
    pub fn from_key(private_key: <S::CryptoSpec as CryptoSpec>::PrivateKey) -> Self {
        let address = private_key.to_address::<S::Address>();
        Self {
            private_key,
            address,
        }
    }
}

/// A list of addresses associated with this wallet
#[derive(Debug, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + DeserializeOwned")]
pub struct AddressList<S: sov_modules_api::Spec> {
    /// All addresses which are known by the wallet. The active address is stored
    /// first in this array
    addresses: Vec<AddressEntry<S>>,
}

impl<S: sov_modules_api::Spec> AddressList<S> {
    /// Get the active address
    pub fn default_address(&self) -> Option<&AddressEntry<S>> {
        self.addresses.first()
    }

    /// Get an address by identifier
    pub fn get_address(&mut self, identifier: &KeyIdentifier<S>) -> Option<&mut AddressEntry<S>> {
        self.addresses
            .iter_mut()
            .find(|entry| entry.matches(identifier))
    }

    /// Activate a key by identifier
    pub fn activate(&mut self, identifier: &KeyIdentifier<S>) -> Option<&AddressEntry<S>> {
        let (idx, _) = self
            .addresses
            .iter()
            .enumerate()
            .find(|(_idx, entry)| entry.matches(identifier))?;
        self.addresses.swap(0, idx);
        self.default_address()
    }

    /// Remove an address from the wallet by identifier
    pub fn remove(&mut self, identifier: &KeyIdentifier<S>) {
        self.addresses.retain(|entry| !entry.matches(identifier));
    }

    /// Add an address to the wallet
    pub fn add(
        &mut self,
        address: S::Address,
        nickname: Option<String>,
        public_key: <S::CryptoSpec as CryptoSpec>::PublicKey,
        location: PathBuf,
    ) {
        let entry = AddressEntry {
            address,
            nickname,
            location,
            pub_key: public_key,
        };
        self.addresses.push(entry);
    }
}

/// An entry in the address list
#[derive(Debug, Serialize, Deserialize)]
#[serde(bound = "S::Address: Serialize + DeserializeOwned")]
pub struct AddressEntry<S: sov_modules_api::Spec> {
    /// The address
    pub address: S::Address,
    /// A user-provided nickname
    pub nickname: Option<String>,
    /// The location of the private key on disk
    pub location: PathBuf,
    /// The public key associated with the address
    #[serde(with = "pubkey_hex")]
    pub pub_key: <S::CryptoSpec as CryptoSpec>::PublicKey,
}

impl<S: sov_modules_api::Spec> AddressEntry<S> {
    /// Check if the address entry matches the given nickname
    pub fn is_nicknamed(&self, nickname: &str) -> bool {
        self.nickname.as_deref() == Some(nickname)
    }

    /// Check if the address entry matches the given identifier
    pub fn matches(&self, identifier: &KeyIdentifier<S>) -> bool {
        match identifier {
            KeyIdentifier::ByNickname { nickname } => self.is_nicknamed(nickname),
            KeyIdentifier::ByAddress { address } => &self.address == address,
        }
    }
}

/// An identifier for a key in the wallet
#[derive(Debug, clap::Subcommand, Clone)]
pub enum KeyIdentifier<S: sov_modules_api::Spec> {
    /// Select a key by nickname
    ByNickname {
        /// The nickname
        nickname: String,
    },
    /// Select a key by its associated address
    ByAddress {
        /// The address
        address: S::Address,
    },
}
impl<S: sov_modules_api::Spec> std::fmt::Display for KeyIdentifier<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyIdentifier::ByNickname { nickname } => nickname.fmt(f),
            KeyIdentifier::ByAddress { address } => address.fmt(f),
        }
    }
}

mod pubkey_hex {
    use core::fmt;
    use std::marker::PhantomData;

    use borsh::{BorshDeserialize, BorshSerialize};
    use hex::{FromHex, ToHex};
    use serde::de::{Error, Visitor};
    use serde::{Deserializer, Serializer};
    use sov_modules_api::PublicKey;
    pub fn serialize<P: PublicKey + BorshSerialize, S>(
        data: &P,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let bytes = data
            .try_to_vec()
            .expect("serialization to vec is infallible");
        let formatted_string = format!("0x{}", bytes.encode_hex::<String>());
        serializer.serialize_str(&formatted_string)
    }

    /// Deserializes a hex string into raw bytes.
    ///
    /// Both, upper and lower case characters are valid in the input string and can
    /// even be mixed (e.g. `f9b4ca`, `F9B4CA` and `f9B4Ca` are all valid strings).
    pub fn deserialize<'de, D, C>(deserializer: D) -> Result<C, D::Error>
    where
        D: Deserializer<'de>,
        C: PublicKey + BorshDeserialize,
    {
        struct HexPubkeyVisitor<S>(PhantomData<S>);

        impl<'de, C: PublicKey + BorshDeserialize> Visitor<'de> for HexPubkeyVisitor<C> {
            type Value = C;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "a hex encoded string")
            }

            fn visit_str<E>(self, data: &str) -> Result<Self::Value, E>
            where
                E: Error,
            {
                let data = data.trim_start_matches("0x");
                let bytes: Vec<u8> = FromHex::from_hex(data).map_err(Error::custom)?;
                C::try_from_slice(&bytes).map_err(Error::custom)
            }

            fn visit_borrowed_str<E>(self, data: &'de str) -> Result<Self::Value, E>
            where
                E: Error,
            {
                let data = data.trim_start_matches("0x");
                let bytes: Vec<u8> = FromHex::from_hex(data).map_err(Error::custom)?;
                C::try_from_slice(&bytes).map_err(Error::custom)
            }
        }

        deserializer.deserialize_str(HexPubkeyVisitor(PhantomData::<C>))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type S = sov_test_utils::TestSpec;

    #[test]
    fn test_private_key_and_address() {
        let private_key_and_address = PrivateKeyAndAddress::<S>::generate();

        let json = serde_json::to_string_pretty(&private_key_and_address).unwrap();

        let decoded: PrivateKeyAndAddress<S> = serde_json::from_str(&json).unwrap();

        assert_eq!(
            private_key_and_address.private_key.pub_key(),
            decoded.private_key.pub_key()
        );
        assert_eq!(private_key_and_address.address, decoded.address);
    }
}
