use sha2::Digest;
use sov_accounts::Accounts;
use sov_attester_incentives::AttesterIncentives;
use sov_bank::{Bank, DEFAULT_TOKEN_DECIMALS};
use sov_blob_storage::BlobStorage;
use sov_chain_state::ChainState;
use sov_modules_api::{CodeCommitmentFor, CryptoSpec, Genesis, Spec};
use sov_operator_incentives::OperatorIncentives;
use sov_prover_incentives::ProverIncentives;
use sov_sequencer_registry::SequencerRegistry;
use sov_uniqueness::Uniqueness;

use crate::runtime::TokenId;
use crate::{TestSpec, TestUser};

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

/// Common config for all the rollup types.
pub struct BasicGenesisConfig<S: Spec> {
    /// The sequencer registry config.
    pub sequencer_registry: <SequencerRegistry<S> as Genesis>::Config,
    /// The operator incentives config.
    pub operator_incentives: <OperatorIncentives<S> as Genesis>::Config,
    /// The attester incentives config.
    pub attester_incentives: <AttesterIncentives<S> as Genesis>::Config,
    /// The prover incentives config.
    pub prover_incentives: <ProverIncentives<S> as Genesis>::Config,
    /// The bank config.
    pub bank: <Bank<S> as Genesis>::Config,
    /// The accounts config.
    pub accounts: <Accounts<S> as Genesis>::Config,
    /// The uniqueness config.
    pub uniqueness: <Uniqueness<S> as Genesis>::Config,
    /// The chain state config.
    pub chain_state: <ChainState<S> as Genesis>::Config,
    /// The blob storage config.
    pub blob_storage: <BlobStorage<S> as Genesis>::Config,
}

/// A convenient high-level representation of a ZK genesis config.
#[derive(derivative::Derivative, Clone)]
#[derivative(Debug(bound = ""))]
struct HighLevelBasicConfig<S: Spec> {
    additional_accounts: Vec<TestUser<S>>,
    gas_token_name: String,
    inner_code_commitment: CodeCommitmentFor<S::InnerZkvm>,
    outer_code_commitment: CodeCommitmentFor<S::OuterZkvm>,
}
