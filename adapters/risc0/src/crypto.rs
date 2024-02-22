//! Cryptography optimized for the Rrisc0 Zkvm.
use std::hash::Hash;
#[cfg(feature = "native")]
use std::str::FromStr;

use borsh::{BorshDeserialize, BorshSerialize};
use ed25519_dalek::{
    Signature as DalekSignature, SigningKey, VerifyingKey as DalekPublicKey, KEYPAIR_LENGTH,
    PUBLIC_KEY_LENGTH,
};
use sha2::Digest;
#[cfg(all(feature = "native", feature = "sov-modules"))]
use sov_modules_api::schemars;
use sov_rollup_interface::crypto::{PublicKeyHex, SigVerificationError};
use sov_rollup_interface::RollupAddress;

/// Defines private key types and operations
#[cfg(feature = "native")]
pub mod private_key {
    use ed25519_dalek::{Signer, SigningKey, KEYPAIR_LENGTH, SECRET_KEY_LENGTH};
    use rand::rngs::OsRng;
    #[cfg(feature = "sov-modules")]
    use sov_modules_api::Address;
    #[cfg(feature = "sov-modules")]
    use sov_rollup_interface::crypto::{PrivateKey, PublicKey};
    use thiserror::Error;

    use super::{Risc0PublicKey, Risc0Signature};

    /// An error that arises during private key deserilization.
    #[derive(Error, Debug)]
    pub enum Risc0PrivateKeyDeserializationError {
        /// An error converting from hex.
        #[error("Hex deserialization error")]
        FromHexError(#[from] hex::FromHexError),
        /// An error indicating that the key pair could not be deserialized.
        #[error("KeyPairError deserialization error")]
        KeyPairError(#[from] ed25519_dalek::SignatureError),
        /// An error indicating that the private key was not the expected length.
        #[error(
            "Invalid private key length: {actual}, expected {secret_key_len} or {keypair_len}"
        )]
        InvalidPrivateKeyLength {
            /// The length of a secret key.
            secret_key_len: usize,
            /// The length of a key pair.
            keypair_len: usize,
            /// The length that was actually found.
            actual: usize,
        },
    }

    /// A private key for the Risc0 signature scheme.
    /// This struct also stores the corresponding public key.
    #[derive(Clone, serde::Serialize, serde::Deserialize)]
    pub struct Risc0PrivateKey {
        key_pair: SigningKey,
    }

    impl Risc0PrivateKey {
        // This is private method and panics if input slice has incorrect length
        fn try_from_keypair(value: &[u8]) -> Result<Self, Risc0PrivateKeyDeserializationError> {
            let value: [u8; KEYPAIR_LENGTH] = value
                .try_into()
                .expect("incorrect usage of `try_from_keypair`, check input length");
            let key_pair = SigningKey::from_keypair_bytes(&value)?;
            Ok(Self { key_pair })
        }

        // This is private method and panics if input slice has incorrect length
        fn try_from_private_key(value: &[u8]) -> Self {
            let value: [u8; SECRET_KEY_LENGTH] = value
                .try_into()
                .expect("incorrect usage of `try_from_private_key`, check input length");
            let key_pair = SigningKey::from_bytes(&value);
            Self { key_pair }
        }
    }

    impl core::fmt::Debug for Risc0PrivateKey {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Risc0PrivateKey")
                .field("public_key", &self.key_pair.verifying_key())
                .field("private_key", &"***REDACTED***")
                .finish()
        }
    }

    impl TryFrom<&[u8]> for Risc0PrivateKey {
        type Error = anyhow::Error;

        fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
            if value.len() == KEYPAIR_LENGTH {
                Self::try_from_keypair(value).map_err(|e| e.into())
            } else if value.len() == SECRET_KEY_LENGTH {
                Ok(Self::try_from_private_key(value))
            } else {
                let err = Err(
                    Risc0PrivateKeyDeserializationError::InvalidPrivateKeyLength {
                        secret_key_len: SECRET_KEY_LENGTH,
                        keypair_len: KEYPAIR_LENGTH,
                        actual: value.len(),
                    },
                );
                err.map_err(|e| e.into())
            }
        }
    }

    impl sov_rollup_interface::crypto::PrivateKey for Risc0PrivateKey {
        type PublicKey = Risc0PublicKey;

        type Signature = Risc0Signature;

        fn generate() -> Self {
            let mut csprng = OsRng;

            Self {
                key_pair: SigningKey::generate(&mut csprng),
            }
        }

        fn pub_key(&self) -> Self::PublicKey {
            Risc0PublicKey {
                pub_key: self.key_pair.verifying_key(),
            }
        }

        fn sign(&self, msg: &[u8]) -> Self::Signature {
            Risc0Signature {
                msg_sig: self.key_pair.sign(msg),
            }
        }
    }

    impl Risc0PrivateKey {
        /// Returns the private key as a hex string.
        pub fn as_hex(&self) -> String {
            hex::encode(self.key_pair.to_bytes())
        }

        /// Tries to decode a hex string into a private key.
        pub fn from_hex(hex: &str) -> anyhow::Result<Self> {
            let bytes = hex::decode(hex)?;
            Self::try_from(&bytes[..])
        }

        /// Returns the address associated with the public key derived from this private key.
        #[cfg(feature = "sov-modules")]
        pub fn default_address(&self) -> Address {
            self.pub_key().to_address::<Address>()
        }
    }

    #[cfg(feature = "arbitrary")]
    impl<'a> arbitrary::Arbitrary<'a> for Risc0PrivateKey {
        fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
            use rand::rngs::StdRng;
            use rand::SeedableRng;

            // it is important to generate the secret deterministically from the arbitrary argument
            // so keys and signatures will be reproducible for a given seed.
            // this unlocks fuzzy replay
            let seed = <[u8; 32]>::arbitrary(u)?;
            let rng = &mut StdRng::from_seed(seed);
            let key_pair = SigningKey::generate(rng);

            Ok(Self { key_pair })
        }
    }

    #[cfg(all(feature = "arbitrary", feature = "native"))]
    impl<'a> arbitrary::Arbitrary<'a> for Risc0PublicKey {
        fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
            #[cfg(not(feature = "sov-modules"))]
            use sov_rollup_interface::crypto::PrivateKey;
            Risc0PrivateKey::arbitrary(u).map(|p| p.pub_key())
        }
    }

    #[cfg(all(feature = "arbitrary", feature = "native"))]
    impl<'a> arbitrary::Arbitrary<'a> for Risc0Signature {
        fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
            #[cfg(not(feature = "sov-modules"))]
            use sov_rollup_interface::crypto::PrivateKey;
            // the secret/public pair is lost; it is impossible to verify this signature
            // to run a verification, generate the keys+payload individually
            let payload_len = u.arbitrary_len::<u8>()?;
            let payload = u.bytes(payload_len)?;
            Risc0PrivateKey::arbitrary(u).map(|s| s.sign(payload))
        }
    }
}

/// The public key of an ed25519 keypair. Wraps the optimized Risc0 fork of the ed25519-dalek crate.
#[cfg_attr(
    all(feature = "native", feature = "sov-modules"),
    derive(schemars::JsonSchema)
)]
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct Risc0PublicKey {
    #[cfg_attr(
        all(feature = "native", feature = "sov-modules"),
        schemars(with = "&[u8]", length(equal = "ed25519_dalek::PUBLIC_KEY_LENGTH"))
    )]
    pub(crate) pub_key: DalekPublicKey,
}

impl sov_rollup_interface::crypto::PublicKey for Risc0PublicKey {
    fn to_address<A: RollupAddress>(&self) -> A {
        let pub_key_hash = {
            let mut hasher = sha2::Sha256::new();
            hasher.update(self.pub_key);
            hasher.finalize().into()
        };
        A::from(pub_key_hash)
    }
}

impl Hash for Risc0PublicKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.pub_key.as_bytes().hash(state);
    }
}

impl BorshDeserialize for Risc0PublicKey {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        let mut buffer = [0; PUBLIC_KEY_LENGTH];
        reader.read_exact(&mut buffer)?;

        let pub_key = DalekPublicKey::from_bytes(&buffer).map_err(map_error)?;

        Ok(Self { pub_key })
    }
}

impl BorshSerialize for Risc0PublicKey {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        writer.write_all(self.pub_key.as_bytes())
    }
}

impl TryFrom<&[u8]> for Risc0PublicKey {
    type Error = anyhow::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        if value.len() == KEYPAIR_LENGTH {
            let mut keypair = [0u8; KEYPAIR_LENGTH];
            keypair.copy_from_slice(value);
            let keypair = SigningKey::from_keypair_bytes(&keypair).map_err(anyhow::Error::msg)?;
            Ok(Self {
                pub_key: keypair.verifying_key(),
            })
        } else if value.len() == PUBLIC_KEY_LENGTH {
            let mut public = [0u8; PUBLIC_KEY_LENGTH];
            public.copy_from_slice(value);
            Ok(Self {
                pub_key: DalekPublicKey::from_bytes(&public).map_err(anyhow::Error::msg)?,
            })
        } else {
            anyhow::bail!("Unexpected public key length")
        }
    }
}

/// An ed25519 signature. Wraps the optimized Risc0 fork of the ed25519-dalek crate.
#[cfg_attr(
    all(feature = "native", feature = "sov-modules"),
    derive(schemars::JsonSchema)
)]
#[derive(PartialEq, Eq, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Risc0Signature {
    /// The inner signature.
    #[cfg_attr(
        all(feature = "native", feature = "sov-modules"),
        schemars(with = "&[u8]", length(equal = "ed25519_dalek::Signature::BYTE_SIZE"))
    )]
    pub msg_sig: DalekSignature,
}

impl BorshDeserialize for Risc0Signature {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        let mut buffer = [0; DalekSignature::BYTE_SIZE];
        reader.read_exact(&mut buffer)?;

        Ok(Self {
            msg_sig: DalekSignature::from_bytes(&buffer),
        })
    }
}

impl BorshSerialize for Risc0Signature {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        writer.write_all(&self.msg_sig.to_bytes())
    }
}

impl TryFrom<&[u8]> for Risc0Signature {
    type Error = anyhow::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self {
            msg_sig: DalekSignature::from_slice(value).map_err(anyhow::Error::msg)?,
        })
    }
}

impl sov_rollup_interface::crypto::Signature for Risc0Signature {
    type PublicKey = Risc0PublicKey;

    fn verify(&self, pub_key: &Self::PublicKey, msg: &[u8]) -> Result<(), SigVerificationError> {
        pub_key
            .pub_key
            .verify_strict(msg, &self.msg_sig)
            .map_err(|e| SigVerificationError::BadSignature(e.to_string()))
    }
}

#[cfg(feature = "native")]
fn map_error(e: ed25519_dalek::SignatureError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e)
}
#[cfg(not(feature = "native"))]
fn map_error(_e: ed25519_dalek::SignatureError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, "Signature error")
}

#[cfg(feature = "native")]
impl FromStr for Risc0PublicKey {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let pk_hex = PublicKeyHex::try_from(s)?;
        Risc0PublicKey::try_from(&pk_hex)
    }
}

#[cfg(feature = "native")]
impl FromStr for Risc0Signature {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = hex::decode(s)?;

        let bytes: ed25519_dalek::ed25519::SignatureBytes = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid signature"))?;

        Ok(Risc0Signature {
            msg_sig: DalekSignature::from_bytes(&bytes),
        })
    }
}

#[cfg(test)]
#[cfg(feature = "native")]
mod tests {
    use sov_rollup_interface::crypto::PrivateKey;

    use super::*;

    #[test]
    fn test_privatekey_serde_bincode() {
        use self::private_key::Risc0PrivateKey;

        let key_pair = Risc0PrivateKey::generate();
        let serialized = bincode::serialize(&key_pair).expect("Serialization to vec is infallible");
        let output = bincode::deserialize::<Risc0PrivateKey>(&serialized)
            .expect("SigningKey is serialized correctly");

        assert_eq!(key_pair.as_hex(), output.as_hex());
    }

    #[test]
    fn test_privatekey_serde_json() {
        use self::private_key::Risc0PrivateKey;

        let key_pair = Risc0PrivateKey::generate();
        let serialized = serde_json::to_vec(&key_pair).expect("Serialization to vec is infallible");
        let output = serde_json::from_slice::<Risc0PrivateKey>(&serialized)
            .expect("Keypair is serialized correctly");

        assert_eq!(key_pair.as_hex(), output.as_hex());
    }
}

impl serde::Serialize for Risc0PublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if serializer.is_human_readable() {
            serde::Serialize::serialize(&PublicKeyHex::from(self), serializer)
        } else {
            serde::Serialize::serialize(&self.pub_key, serializer)
        }
    }
}

impl<'de> serde::Deserialize<'de> for Risc0PublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if deserializer.is_human_readable() {
            let pub_key_hex: PublicKeyHex = serde::Deserialize::deserialize(deserializer)?;
            Ok(Risc0PublicKey::try_from(&pub_key_hex).map_err(serde::de::Error::custom)?)
        } else {
            let pub_key: DalekPublicKey = serde::Deserialize::deserialize(deserializer)?;
            Ok(Risc0PublicKey { pub_key })
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_pub_key_json() {
        let pub_key_hex: PublicKeyHex =
            "022e229198d957bf0c0a504e7d7bcec99a1d62cccc7861ed2452676ad0323ad8"
                .try_into()
                .unwrap();

        let pub_key = Risc0PublicKey::try_from(&pub_key_hex).unwrap();
        let pub_key_str: String = serde_json::to_string(&pub_key).unwrap();

        assert_eq!(
            pub_key_str,
            r#""022e229198d957bf0c0a504e7d7bcec99a1d62cccc7861ed2452676ad0323ad8""#
        );

        let deserialized: Risc0PublicKey = serde_json::from_str(&pub_key_str).unwrap();
        assert_eq!(deserialized, pub_key);
    }
}

impl From<&Risc0PublicKey> for PublicKeyHex {
    fn from(pub_key: &Risc0PublicKey) -> Self {
        let hex = hex::encode(pub_key.pub_key.as_bytes());
        Self { hex }
    }
}

impl TryFrom<&PublicKeyHex> for Risc0PublicKey {
    type Error = anyhow::Error;

    fn try_from(pub_key: &PublicKeyHex) -> Result<Self, Self::Error> {
        let bytes = hex::decode(&pub_key.hex)?;

        let bytes: [u8; PUBLIC_KEY_LENGTH] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid public key size"))?;

        let pub_key = DalekPublicKey::from_bytes(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid public key"))?;

        Ok(Risc0PublicKey { pub_key })
    }
}

#[cfg(test)]
#[cfg(feature = "native")]
mod hex_tests {
    use sov_rollup_interface::crypto::PrivateKey;

    use super::*;
    use crate::crypto::private_key::Risc0PrivateKey;

    #[test]
    fn test_pub_key_hex() {
        let pub_key = Risc0PrivateKey::generate().pub_key();
        let pub_key_hex = PublicKeyHex::from(&pub_key);
        let converted_pub_key = Risc0PublicKey::try_from(&pub_key_hex).unwrap();
        assert_eq!(pub_key, converted_pub_key);
    }

    #[test]
    fn test_pub_key_hex_str() {
        let key = "022e229198d957bf0c0a504e7d7bcec99a1d62cccc7861ed2452676ad0323ad8";
        let pub_key_hex_lower: PublicKeyHex = key.try_into().unwrap();
        let pub_key_hex_upper: PublicKeyHex = key.to_uppercase().try_into().unwrap();

        let pub_key_lower = Risc0PublicKey::try_from(&pub_key_hex_lower).unwrap();
        let pub_key_upper = Risc0PublicKey::try_from(&pub_key_hex_upper).unwrap();

        assert_eq!(pub_key_lower, pub_key_upper)
    }
}
