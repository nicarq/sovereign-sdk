use sha2::Digest;
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
        TokenId::try_from(
            <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::Hasher::digest(self.to_string())
                .as_slice(),
        )
        .unwrap()
    }
}

#[test]
fn test_display_token_name() {
    let token_name = TestTokenName::new("test".to_string());
    assert_eq!("TestToken(test)", token_name.to_string());
}
