use sha2::Digest;
use sov_bank::DEFAULT_TOKEN_DECIMALS;
use sov_modules_api::{CryptoSpec, Spec};

use crate::runtime::TokenId;
use crate::TestSpec;

/// Utilities for testing a runtime in the optimistic execution context.
pub mod optimistic;

/// Utilities for testing a runtime in the ZK execution context.
pub mod zk;

/// A wrapper around a string that can be used to easily identify a test token.
#[derive(Debug, Eq, Hash, Clone, PartialEq, derive_more::Display)]
#[display("TestToken({})", self.0)]
pub struct TestTokenName(
    /// The name of the token. Can be any human-readable string.
    pub String,
);

impl TestTokenName {
    /// Creates a new token name from a string.
    pub fn new(name: String) -> Self {
        Self(name)
    }

    /// Returns the ID of the token.
    pub fn id(&self) -> TokenId {
        let mut bytes: [u8; 32] =
            <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::Hasher::digest(self.to_string())
                .as_slice()
                .try_into()
                .unwrap();
        bytes[31] = DEFAULT_TOKEN_DECIMALS;
        TokenId::from(bytes)
    }
}

#[test]
fn test_display_token_name() {
    let token_name = TestTokenName::new("test".to_string());
    assert_eq!("TestToken(test)", token_name.to_string());
}
