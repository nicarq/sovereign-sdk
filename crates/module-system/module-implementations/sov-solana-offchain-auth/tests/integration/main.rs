#![allow(unused_imports)]
use std::sync::Arc;

use sov_mock_da::{BlockProducingConfig, MockAddress, MockDaService};
use sov_mock_zkvm::MockZkvm;
use sov_modules_api::capabilities::TransactionAuthenticator;
use sov_modules_api::prelude::*;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{FullyBakedTx, RawTx, Runtime, Spec};
use sov_modules_stf_blueprint::GenesisParams;
use sov_paymaster::PaymasterConfig;
use sov_rollup_interface::execution_mode::Native;
use sov_solana_offchain_auth::capabilities::{
    SolanaOffchainAuthenticator, SolanaOffchainAuthenticatorInput, SolanaOffchainAuthenticatorTrait,
};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::Runtime as _;
use sov_test_utils::test_rollup::{GenesisSource, RollupBuilder, TestRollup};
use sov_test_utils::{generate_runtime, RtAgnosticBlueprint, TestSpec};
use sov_value_setter::ValueSetterConfig;
use tempfile::tempdir;

// Generate the test runtime with Solana offchain authenticator
generate_runtime! {
    name: SolanaTestRuntime,
    modules: [
        value_setter: sov_value_setter::ValueSetter<S>,
        paymaster: sov_paymaster::Paymaster<S>,
    ],
    operating_mode: sov_modules_api::runtime::OperatingMode::Optimistic,
    minimal_genesis_config_type: sov_test_utils::runtime::genesis::optimistic::config::MinimalOptimisticGenesisConfig<S>,
    runtime_trait_impl_bounds: [],
    kernel_type: sov_kernels::soft_confirmations::SoftConfirmationsKernel<'a, S>,
    auth_type: SolanaOffchainAuthenticator<S, Self>,
    auth_call_wrapper: |call| call,
}

impl<S: Spec> SolanaOffchainAuthenticatorTrait<S> for SolanaTestRuntime<S> {
    fn add_solana_offchain_auth(tx: RawTx) -> <Self::Auth as TransactionAuthenticator<S>>::Input {
        SolanaOffchainAuthenticatorInput::SolanaOffchain(tx)
    }
}

type RT = SolanaTestRuntime<TestSpec>;

async fn create_test_rollup() -> anyhow::Result<TestRollup<RtAgnosticBlueprint<TestSpec, RT>>> {
    // Create genesis config
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let admin = genesis_config.additional_accounts()[0].clone();

    let rt_genesis_config = <RT as Runtime<TestSpec>>::GenesisConfig::from_minimal_config(
        genesis_config.clone().into(),
        ValueSetterConfig {
            admin: admin.address(),
        },
        PaymasterConfig::default(),
    );

    let genesis_params = GenesisParams {
        runtime: rt_genesis_config,
    };

    let dir = Arc::new(tempdir()?);
    let seq_da_address = genesis_params
        .runtime
        .sequencer_registry
        .sequencer_config
        .seq_da_address;

    // Build the test rollup
    let rollup = RollupBuilder::<RtAgnosticBlueprint<TestSpec, RT>>::new(
        GenesisSource::CustomParams(genesis_params),
        BlockProducingConfig::Manual,
        3, // finalization blocks
    )
    .set_config(|c| {
        c.storage = dir.clone();
        c.automatic_batch_production = false;
        c.max_batch_size_bytes = 1024 * 1024; // 1MB
        c.blob_processing_timeout_secs = 60;
    })
    .set_da_config(|c| c.sender_address = seq_da_address)
    .set_persistent_da()
    .start()
    .await?;

    Ok(rollup)
}

#[tokio::test(flavor = "multi_thread")]
async fn test_rollup_initialization() {
    // Just test that we can create a rollup with the Solana authenticator
    let rollup = create_test_rollup().await;
    assert!(
        rollup.is_ok(),
        "Failed to create test rollup: {:?}",
        rollup.err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_submit_solana_offchain_transaction() {
    let _rollup = create_test_rollup().await.expect("Failed to create rollup");

    // TODO: This is where we'll submit pre-signed Solana offchain transactions
    // For now, just verify the rollup is running

    // Check that the rollup is initialized
    // let latest_block = rollup.get_last_published_slot_number().await;
    // assert_eq!(latest_block, 0, "Expected initial slot to be 0");
}

#[test]
fn test_auth_wrapper() {
    // Test that the auth wrapper correctly identifies transaction types
    let raw_tx = RawTx::new(vec![1, 2, 3]);

    // Test standard auth
    let standard_auth = <RT as Runtime<TestSpec>>::Auth::add_standard_auth(raw_tx.clone());
    assert!(matches!(
        standard_auth,
        SolanaOffchainAuthenticatorInput::Standard(_)
    ));

    // Test Solana offchain auth
    let solana_auth = RT::add_solana_offchain_auth(raw_tx);
    assert!(matches!(
        solana_auth,
        SolanaOffchainAuthenticatorInput::SolanaOffchain(_)
    ));
}
