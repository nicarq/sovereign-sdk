//! Cryptography optimized for the Ligetron zkVM.
//! 
//! This module provides ed25519 signature and public key implementations
//! that are compatible with the Sovereign SDK's crypto traits.

use std::hash::Hash;

use borsh::{BorshDeserialize, BorshSerialize};
use ed25519_dalek::{
    Signature as DalekSignature, VerifyingKey as DalekPublicKey, PUBLIC_KEY_LENGTH,
    Verifier, // <-- bring the trait into scope
};
use sov_rollup_interface::crypto::SigVerificationError;
use sov_rollup_interface::reexports::schemars::{self, JsonSchema};
use sov_rollup_interface::sov_universal_wallet::UniversalWallet;

/// Defines private key types and operations for native environments
#[cfg(feature = "native")]
pub mod private_key {
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;

    use super::{LigetronPublicKey, LigetronSignature};

    /// A private key for the Ligetron signature scheme.
    /// This struct also stores the corresponding public key.
    #[derive(Clone, serde::Serialize, serde::Deserialize)]
    pub struct LigetronPrivateKey {
        key_pair: SigningKey,
    }

    impl core::fmt::Debug for LigetronPrivateKey {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("LigetronPrivateKey")
                .field("public_key", &self.key_pair.verifying_key())
                .field("private_key", &"***REDACTED***")
                .finish()
        }
    }

    impl sov_rollup_interface::crypto::PrivateKey for LigetronPrivateKey {
        type PublicKey = LigetronPublicKey;
        type Signature = LigetronSignature;

    fn generate() -> Self {
        use rand::RngCore;
        let mut secret_bytes = [0u8; 32];
        OsRng.fill_bytes(&mut secret_bytes);
        Self {
            key_pair: SigningKey::from_bytes(&secret_bytes),
        }
    }

        fn pub_key(&self) -> Self::PublicKey {
            LigetronPublicKey {
                pub_key: self.key_pair.verifying_key(),
            }
        }

        fn sign(&self, msg: &[u8]) -> Self::Signature {
            LigetronSignature {
                msg_sig: self.key_pair.sign(msg),
            }
        }
    }

    impl LigetronPrivateKey {
        /// Returns the private key as a hex string.
        pub fn as_hex(&self) -> String {
            hex::encode(self.key_pair.to_bytes())
        }

        /// Create a private key from hex string.
        pub fn from_hex(hex_str: &str) -> Result<Self, hex::FromHexError> {
            let bytes = hex::decode(hex_str)?;
            let key_bytes: [u8; 32] = bytes.try_into()
                .map_err(|_| hex::FromHexError::InvalidStringLength)?;
            let key_pair = SigningKey::from_bytes(&key_bytes);
            Ok(Self { key_pair })
        }
    }

    #[cfg(feature = "arbitrary")]
    mod arbitrary_impls {
        use proptest::prelude::{any, BoxedStrategy};
        use proptest::strategy::Strategy;
        use rand::rngs::StdRng;
        use rand::SeedableRng;
        use sov_rollup_interface::crypto::PrivateKey;

        use super::*;

        impl proptest::arbitrary::Arbitrary for LigetronPrivateKey {
            type Parameters = ();
            type Strategy = BoxedStrategy<Self>;

            fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
                any::<u64>()
                    .prop_map(|seed| {
                        let mut rng = StdRng::seed_from_u64(seed);
                        let key_pair = SigningKey::generate(&mut rng);
                        Self { key_pair }
                    })
                    .boxed()
            }
        }
    }
}

/// A public key in the Ligetron signature scheme.
#[derive(
    PartialEq,
    Eq,
    Hash,
    Clone,
    Debug,
    serde::Serialize,
    serde::Deserialize,
    UniversalWallet,
)]
pub struct LigetronPublicKey {
    #[sov_wallet(as_ty = "[u8; 32]")]
    pub(crate) pub_key: DalekPublicKey,
}

impl JsonSchema for LigetronPublicKey {
    fn schema_name() -> String {
        "LigetronPublicKey".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        String::json_schema(gen)
    }
}

impl LigetronPublicKey {
    /// Get the raw bytes of the public key.
    pub fn as_bytes(&self) -> &[u8; PUBLIC_KEY_LENGTH] {
        self.pub_key.as_bytes()
    }

    /// Create a public key from raw bytes.
    pub fn from_bytes(bytes: &[u8; PUBLIC_KEY_LENGTH]) -> Result<Self, ed25519_dalek::SignatureError> {
        Ok(Self {
            pub_key: DalekPublicKey::from_bytes(bytes)?,
        })
    }

    /// Create a public key from hex string.
    pub fn from_hex(hex_str: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let bytes = hex::decode(hex_str)?;
        let key_bytes: [u8; PUBLIC_KEY_LENGTH] = bytes.try_into()
            .map_err(|_| "Invalid public key length")?;
        Ok(Self::from_bytes(&key_bytes)?)
    }

    /// Convert the public key to hex string.
    pub fn to_hex(&self) -> String {
        hex::encode(self.as_bytes())
    }
}

impl sov_rollup_interface::crypto::PublicKey for LigetronPublicKey {
    fn credential_id(&self) -> sov_rollup_interface::crypto::CredentialId {
        let hex_data = sov_rollup_interface::common::HexString(*self.as_bytes());
        sov_rollup_interface::crypto::CredentialId(hex_data)
    }
}

impl BorshSerialize for LigetronPublicKey {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        writer.write_all(self.pub_key.as_bytes())
    }
}

impl BorshDeserialize for LigetronPublicKey {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        let mut bytes = [0u8; PUBLIC_KEY_LENGTH];
        reader.read_exact(&mut bytes)?;
        let pub_key = DalekPublicKey::from_bytes(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(Self { pub_key })
    }
}

impl TryFrom<[u8; 32]> for LigetronPublicKey {
    type Error = ed25519_dalek::SignatureError;

    fn try_from(bytes: [u8; 32]) -> Result<Self, Self::Error> {
        Self::from_bytes(&bytes)
    }
}

/// A signature in the Ligetron signature scheme.
#[derive(
    PartialEq,
    Eq,
    Debug,
    Clone,
    serde::Serialize,
    serde::Deserialize,
    UniversalWallet,
)]
pub struct LigetronSignature {
    #[sov_wallet(as_ty = "[u8; 64]")]
    pub(crate) msg_sig: DalekSignature,
}

impl JsonSchema for LigetronSignature {
    fn schema_name() -> String {
        "LigetronSignature".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        String::json_schema(gen)
    }
}

impl LigetronSignature {
    /// Get the raw bytes of the signature.
    pub fn as_bytes(&self) -> [u8; 64] {
        self.msg_sig.to_bytes()
    }

    /// Create a signature from raw bytes.
    pub fn from_bytes(bytes: &[u8; 64]) -> Self {
        Self {
            msg_sig: DalekSignature::from_bytes(bytes),
        }
    }

    /// Create a signature from hex string.
    pub fn from_hex(hex_str: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let bytes = hex::decode(hex_str)?;
        let sig_bytes: [u8; 64] = bytes.try_into()
            .map_err(|_| "Invalid signature length")?;
        Ok(Self::from_bytes(&sig_bytes))
    }

    /// Convert the signature to hex string.
    pub fn to_hex(&self) -> String {
        hex::encode(self.as_bytes())
    }
}

impl TryFrom<&[u8]> for LigetronSignature {
    type Error = anyhow::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self {
            msg_sig: DalekSignature::from_slice(value).map_err(anyhow::Error::msg)?,
        })
    }
}

impl sov_rollup_interface::crypto::Signature for LigetronSignature {
    type PublicKey = LigetronPublicKey;

    fn verify(&self, public_key: &Self::PublicKey, msg: &[u8]) -> Result<(), SigVerificationError> {
        // use ed25519_dalek::Verifier;
        
        public_key.pub_key
            .verify_strict(msg, &self.msg_sig)
            .map_err(|e| SigVerificationError {
                error: e.to_string(),
            })
    }
}

impl BorshSerialize for LigetronSignature {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        writer.write_all(&self.as_bytes())
    }
}

impl BorshDeserialize for LigetronSignature {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        let mut bytes = [0u8; 64];
        reader.read_exact(&mut bytes)?;
        Ok(Self {
            msg_sig: DalekSignature::from_bytes(&bytes),
        })
    }
}

#[cfg(feature = "arbitrary")]
mod arbitrary_impls {
    use proptest::prelude::{any, BoxedStrategy};
    use proptest::strategy::Strategy;

    use super::*;

    impl proptest::arbitrary::Arbitrary for LigetronPublicKey {
        type Parameters = ();
        type Strategy = BoxedStrategy<Self>;

        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            any::<[u8; 32]>()
                .prop_filter_map("valid public key", |bytes| {
                    DalekPublicKey::from_bytes(&bytes).ok().map(|pub_key| Self { pub_key })
                })
                .boxed()
        }
    }

    impl proptest::arbitrary::Arbitrary for LigetronSignature {
        type Parameters = ();
        type Strategy = BoxedStrategy<Self>;

        fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
            any::<[u8; 64]>()
                .prop_map(|bytes| Self {
                    msg_sig: DalekSignature::from_bytes(&bytes),
                })
                .boxed()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sov_rollup_interface::crypto::{PrivateKey, PublicKey, Signature};

    #[cfg(feature = "native")]
    #[test]
    fn test_key_generation_and_signing() {
        use crate::crypto::private_key::LigetronPrivateKey;

        let private_key = LigetronPrivateKey::generate();
        let public_key = private_key.pub_key();
        
        let message = b"test message";
        let signature = private_key.sign(message);
        
        // Verify signature
        assert!(signature.verify(&public_key, message).is_ok());
        
        // Verify wrong message fails
        let wrong_message = b"wrong message";
        assert!(signature.verify(&public_key, wrong_message).is_err());
    }

    #[test]
    fn test_public_key_serialization() {
        // Generate a valid key pair using ed25519_dalek directly (works without native feature)
        use rand::RngCore;
        let mut rng = rand::rngs::OsRng;
        let mut secret_bytes = [0u8; 32];
        rng.fill_bytes(&mut secret_bytes);
        let sk = ed25519_dalek::SigningKey::from_bytes(&secret_bytes);
        let public_key = LigetronPublicKey { pub_key: sk.verifying_key() };
        
        // Test borsh serialization
        let serialized = borsh::to_vec(&public_key).unwrap();
        let deserialized: LigetronPublicKey = borsh::from_slice(&serialized).unwrap();
        assert_eq!(public_key, deserialized);
        
        // Test hex conversion
        let hex_str = public_key.to_hex();
        let from_hex = LigetronPublicKey::from_hex(&hex_str).unwrap();
        assert_eq!(public_key, from_hex);
    }

    #[test]
    fn test_signature_serialization() {
        // Generate a valid signature using ed25519_dalek directly (works without native feature)
        use rand::RngCore;
        use ed25519_dalek::Signer;
        let mut rng = rand::rngs::OsRng;
        let mut secret_bytes = [0u8; 32];
        rng.fill_bytes(&mut secret_bytes);
        let sk = ed25519_dalek::SigningKey::from_bytes(&secret_bytes);
        let message = b"test message for signature";
        let signature = LigetronSignature { msg_sig: sk.sign(message) };
        
        // Test borsh serialization
        let serialized = borsh::to_vec(&signature).unwrap();
        let deserialized: LigetronSignature = borsh::from_slice(&serialized).unwrap();
        assert_eq!(signature, deserialized);
        
        // Test hex conversion
        let hex_str = signature.to_hex();
        let from_hex = LigetronSignature::from_hex(&hex_str).unwrap();
        assert_eq!(signature, from_hex);
    }

    #[test]
    fn test_credential_id() {
        // Generate a valid key pair using ed25519_dalek directly (works without native feature)
        use rand::RngCore;
        let mut rng = rand::rngs::OsRng;
        let mut secret_bytes = [0u8; 32];
        rng.fill_bytes(&mut secret_bytes);
        let sk = ed25519_dalek::SigningKey::from_bytes(&secret_bytes);
        let public_key = LigetronPublicKey { pub_key: sk.verifying_key() };
        let credential_id = public_key.credential_id();
        
        // Credential ID should be deterministic
        let credential_id2 = public_key.credential_id();
        assert_eq!(credential_id, credential_id2);
        
        // Should match expected hex format (32 bytes = 64 hex chars + 0x prefix)
        let credential_str = credential_id.to_string();
        assert!(credential_str.starts_with("0x"));
        assert_eq!(credential_str.len(), 66); // 0x + 64 hex chars
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_private_key_hex_conversion() {
        use crate::crypto::private_key::LigetronPrivateKey;

        let private_key = LigetronPrivateKey::generate();
        let hex_str = private_key.as_hex();
        let from_hex = LigetronPrivateKey::from_hex(&hex_str).unwrap();
        
        // Should produce the same public key
        assert_eq!(private_key.pub_key(), from_hex.pub_key());
    }

    #[test]
    fn test_invalid_public_key() {
        // Test invalid length
        let result = LigetronPublicKey::from_hex("deadbeef");
        assert!(result.is_err());
        
        // Test invalid hex
        let result = LigetronPublicKey::from_hex("invalid_hex_string_with_correct_length_but_bad_chars");
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_signature() {
        // Test invalid length
        let result = LigetronSignature::from_hex("deadbeef");
        assert!(result.is_err());
        
        // Test invalid hex
        let result = LigetronSignature::from_hex("invalid_hex_string_with_correct_length_but_bad_chars_and_more_chars_to_reach_128_chars_total_length");
        assert!(result.is_err());
    }
}
