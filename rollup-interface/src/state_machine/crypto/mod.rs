//! Defines useful cryptographic primitives that are needed by all sovereign-sdk rollups.
mod simple_hasher;

pub use simple_hasher::NoOpHasher;
mod signatures;
pub use signatures::*;

/// Wrapper around hash value.
#[cfg_attr(feature = "native", derive(schemars::JsonSchema))]
#[derive(
    borsh::BorshDeserialize,
    borsh::BorshSerialize,
    serde::Serialize,
    serde::Deserialize,
    Debug,
    PartialEq,
    Clone,
)]
pub struct Hash(pub [u8; 32]);

#[cfg(feature = "native")]
impl core::str::FromStr for Hash {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim_start_matches("0x");
        let decoded = <[u8; 32] as hex::FromHex>::from_hex(s)?;
        Ok(Self(decoded))
    }
}

#[cfg(feature = "native")]
impl core::fmt::Display for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x{}", hex::encode(self.0))
    }
}

#[cfg(test)]
mod tests {
    use core::str::FromStr;

    use proptest::prelude::*;

    use super::*;

    fn check_str_and_back(hash: Hash) {
        let s1 = hash.to_string();
        let s2 = s1.replace("0x", "");
        let hash2 = Hash::from_str(&s1).unwrap();
        let hash3 = Hash::from_str(&s2).unwrap();
        assert_eq!(hash, hash2);
        assert_eq!(hash, hash3);
    }

    #[test]
    fn test_hash_str_and_back_simple() {
        for i in 0..1 {
            let hash = Hash([i as u8; 32]);
            check_str_and_back(hash);
        }
    }

    proptest! {
        #[test]
        fn test_arbitrary_hash_str_and_back(input in prop::array::uniform32(any::<u8>())) {
            let hash = Hash(input);
            check_str_and_back(hash);
        }
    }
}
