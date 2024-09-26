//! Project level utilities that are used for testing the different crates of Sovereign SDK.
//!
//! WARNING: This crate is **NOT** intended to be used in production code. This is a testing utility crate.

#![deny(missing_docs)]

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

pub use api_client::ApiClient;
use borsh::BorshSerialize;
pub use evm::simple_smart_contract::SimpleStorageContract;
pub use generators::MessageGenerator;
pub use interface::*;
use serde::{Deserialize, Serialize};
pub use sov_db::schema::SchemaBatch;
pub use sov_mock_da::verifier::MockDaSpec;
pub use sov_mock_zkvm::MockZkVerifier;
use sov_modules_api::macros::config_value;
use sov_modules_api::transaction::{PriorityFeeBips, Transaction, TxDetails, UnsignedTransaction};
pub use sov_modules_api::EncodeCall;
use sov_modules_api::{CryptoSpec, Spec};
pub use sov_modules_stf_blueprint::{get_gas_used, SkippedReason};
use sov_modules_stf_blueprint::{BatchReceipt, StfBlueprint};
use sov_rollup_interface::execution_mode::{Native, Zk};
pub use sov_state::ProverStorage;

use crate::runtime::BasicKernel;

mod api_client;

mod evm;

/// Utilities for generating test data.
pub mod generators;

/// Utilities for writing integration tests against ledger APIs (both Rust API and REST APIs).
pub mod ledger_db;

/// Utilities for logging tests.
pub mod logging;

/// Utilities for testing the runtime.
pub mod runtime;

/// Utilities for testing the sequencer.
pub mod sequencer;
/// Utilities for testing that require [`ProverStorage`].
pub mod storage;

/// Utilities that specify an interface for testing.
pub mod interface;

/// The default test spec. Uses a [`MockZkVerifier`] for both inner and outer vm verification.
/// Uses [`sov_mock_zkvm::MockZkvmCryptoSpec`] for cryptographic primitives.
pub type TestSpec =
    sov_modules_api::default_spec::DefaultSpec<MockZkVerifier, MockZkVerifier, Native>;
/// The default test spec for ZK. Uses a [`MockZkVerifier`] for both inner and outer vm verification.
pub type ZkTestSpec =
    sov_modules_api::default_spec::DefaultSpec<MockZkVerifier, MockZkVerifier, Zk>;
/// The default address type. This is the [`sov_modules_api::RollupAddress`] type defined by the [`TestSpec`].
pub type TestAddress = <TestSpec as Spec>::Address;
/// The default test crypto spec type. This is the [`CryptoSpec`] type defined by the [`TestSpec`].
pub type TestCryptoSpec = <TestSpec as Spec>::CryptoSpec;
/// The default private key type. This is the [`sov_rollup_interface::crypto::PrivateKey`] type defined by the [`TestSpec`].
pub type TestPrivateKey = <TestCryptoSpec as CryptoSpec>::PrivateKey;
/// The default public key type. This is the [`sov_rollup_interface::crypto::PublicKey`] type defined by the [`TestCryptoSpec`].
pub type TestPublicKey = <TestCryptoSpec as CryptoSpec>::PublicKey;
/// The default signature type. This is the [`sov_rollup_interface::crypto::Signature`] type defined by the [`TestCryptoSpec`].
pub type TestSignature = <TestCryptoSpec as CryptoSpec>::Signature;
/// The default hasher type. This is the hasher type
/// ([`sov_rollup_interface::reexports::digest::Digest`]) defined by the
/// [`TestCryptoSpec`].
pub type TestHasher = <TestCryptoSpec as CryptoSpec>::Hasher;
/// The default storage spec type. Uses a [`TestHasher`] for hashing.
pub type TestStorageSpec = sov_state::DefaultStorageSpec<TestHasher>;
/// The default STF blueprint type. Uses [`MockDaSpec`] for DA and custom kernel.
pub type TestStfBlueprintWithKernel<RT, K, S> = StfBlueprint<S, MockDaSpec, RT, K>;
/// The default STF blueprint type. Uses [`MockDaSpec`] for DA and [`BasicKernel`] for kernel.
pub type TestStfBlueprint<RT, S> = StfBlueprint<S, MockDaSpec, RT, BasicKernel<S, MockDaSpec>>;
/// The default [`sov_db::storage_manager::NativeStorageManager`], that can be used with [`ProverStorage`] and [`TestStorageSpec`].
pub type TestStorageManager =
    sov_db::storage_manager::NativeStorageManager<MockDaSpec, ProverStorage<TestStorageSpec>>;
// --- Blessed test parameters ---

// Blessed gas parameters

/// The default max fee to set for a transaction. This should be enough to be able to execute most standard transactions for the test rollup.
pub const TEST_DEFAULT_MAX_FEE: u64 = 100_000_000;
/// The default gas limit to set for a transaction. This is an optional parameter.
/// This value should be high enough to be able to execute most standard transactions for the test rollup.
pub const TEST_DEFAULT_GAS_LIMIT: [u64; 2] = [1_000_000, 1_000_000];
/// The default amount of tokens that should be staked by a user (prover, sequencer, etc.). This value is roughly equal to the
/// max fee for a transaction because sequencers need to pre-emptively pay for all transactions' pre-execution checks using their stake.
pub const TEST_DEFAULT_USER_STAKE: [u64; 2] = [500_000, 500_000];
/// The default amount of tokens that should be in the user's bank account. This amount should always be higher than [`TEST_DEFAULT_MAX_FEE`] and
/// [`TEST_DEFAULT_USER_STAKE`]. This value is set so that the user can send a dozen transactions without having to refill its bank account.
pub const TEST_DEFAULT_USER_BALANCE: u64 = 1_000_000_000_000;
/// The default max priority fee to set for a transaction. We are setting this value to zero to avoid having to do
/// priority fee accounting in the tests. If a test needs to test sequencer rewards, it should set the transaction priority fee
/// to a non-zero value.
pub const TEST_DEFAULT_MAX_PRIORITY_FEE: PriorityFeeBips = PriorityFeeBips::from_percentage(0);

// --- End Blessed gas parameters (used for testing) ---

// Blessed rollup constants
// Constants used in the genesis configuration of the test runtime

// --- Attester incentives constants ---
/// The default max attested height at the genesis of the rollup. This is the height that contains the highest attestation
/// for the rollup. This value is set to zero in tests because the rollup always starts at zeroth height.
pub const TEST_MAX_ATTESTED_HEIGHT: u64 = 0;
/// The default finalized height of the light client. This value should always be below the [`TEST_MAX_ATTESTED_HEIGHT`].
/// This value is set to zero in tests because the rollup always starts at zeroth height. This value should be manually
/// updated for now because light clients are not yet implemented.
pub const TEST_LIGHT_CLIENT_FINALIZED_HEIGHT: u64 = 0;
/// The default rollup finality period. Used by the [`sov_attester_incentives::AttesterIncentives`] module to determine the
/// range of heights that are eligible for attestations.
pub const TEST_ROLLUP_FINALITY_PERIOD: u64 = 5;
/// The default name to use for the gas token.
pub const TEST_GAS_TOKEN_NAME: &str = "TestGasToken";

/// Generates a default [`TxDetails`] for testing.
pub(crate) fn default_test_tx_details<S: Spec>() -> TxDetails<S> {
    TxDetails {
        max_priority_fee_bips: TEST_DEFAULT_MAX_PRIORITY_FEE,
        max_fee: TEST_DEFAULT_MAX_FEE,
        gas_limit: None,
        chain_id: config_value!("CHAIN_ID"),
    }
}

/// Creates signed transaction with default test parameters from serializable RuntimeCallMessage.
pub fn default_test_signed_transaction<T: BorshSerialize>(
    key: &TestPrivateKey,
    msg: &T,
    nonce: u64,
) -> Transaction<TestSpec> {
    let tx_details = default_test_tx_details::<TestSpec>();

    Transaction::<TestSpec>::new_signed_tx(
        key,
        UnsignedTransaction::new(
            borsh::to_vec(&msg).unwrap(),
            tx_details.chain_id,
            tx_details.max_priority_fee_bips,
            tx_details.max_fee,
            nonce,
            tx_details.gas_limit,
        ),
    )
}

/// An implementation of [`sov_rollup_interface::stf::TxReceiptContents`] for testing. TestTxReceiptContents uses
/// a `u32` as the receipt contents.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct TestTxReceiptContents;

impl sov_rollup_interface::stf::TxReceiptContents for TestTxReceiptContents {
    type Skipped = u32;
    type Reverted = u32;
    type Successful = u32;
}

/// Simplified AtomicU64 for using in tests.
#[derive(Clone)]
pub struct AtomicNumber {
    num: Arc<AtomicU64>,
}

impl AtomicNumber {
    /// Create a new AtomicNumber
    pub fn new(num: u64) -> Self {
        Self {
            num: Arc::new(AtomicU64::new(num)),
        }
    }

    /// Get the current value of the AtomicNumber
    pub fn get(&self) -> u64 {
        self.num.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Add a value to the AtomicNumber
    pub fn add(&self, value: u64) {
        self.num
            .fetch_add(value, std::sync::atomic::Ordering::SeqCst);
    }

    /// Subtract a value from the AtomicNumber
    pub fn sub(&self, value: u64) {
        self.num
            .fetch_sub(value, std::sync::atomic::Ordering::SeqCst);
    }
}
