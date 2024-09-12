use sha2::Digest;
use sov_chain_state::ChainStateConfig;
use sov_kernels::basic::BasicKernelGenesisConfig;
use sov_mock_zkvm::MockCodeCommitment;
use sov_modules_api::{CryptoSpec, OperatingMode, Spec};
use sov_rollup_interface::da::DaSpec;

use crate::runtime::TokenId;
use crate::TestSpec;

/// Utilities for testing a runtime in the optimistic execution context.
pub mod optimistic;

/// Utilities for testing a runtime in the ZK execution context.
pub mod zk;

/// A wrapper around a string that can be used to easily identify a test token.
#[derive(Debug, Eq, Hash, Clone, PartialEq, derive_more::Display)]
#[display(fmt = "TestToken({})", "self.0")]
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

/// Short function to generate default kernel genesis config
pub fn default_basic_kernel_genesis<Da: DaSpec>(
    operating_mode: OperatingMode,
) -> BasicKernelGenesisConfig<TestSpec, Da> {
    BasicKernelGenesisConfig::<TestSpec, Da> {
        chain_state: ChainStateConfig {
            current_time: Default::default(),
            operating_mode,
            inner_code_commitment: MockCodeCommitment::default(),
            outer_code_commitment: MockCodeCommitment::default(),
            genesis_da_height: 0,
        },
    }
}

#[test]
fn test_display_token_name() {
    let token_name = TestTokenName::new("test".to_string());
    assert_eq!("TestToken(test)", token_name.to_string());
}
