#![allow(unused_imports)]
use std::sync::Arc;
use std::str::FromStr;

use sov_bank::{Amount, Coins, TokenId};
use sov_mock_da::{BlockProducingConfig, MockAddress, MockDaService};
use sov_mock_zkvm::crypto::Ed25519Signature;
use sov_mock_zkvm::MockZkvm;
use sov_modules_api::capabilities::{TransactionAuthenticator, UniquenessData};
use sov_modules_api::prelude::*;
use sov_modules_api::transaction::{Transaction, UnsignedTransaction};
use sov_modules_api::{FullyBakedTx, RawTx, Runtime, Spec};
use sov_modules_stf_blueprint::GenesisParams;
use sov_paymaster::PaymasterConfig;
use sov_rollup_interface::execution_mode::Native;
use sov_solana_offchain_auth::utils::make_preamble_for_message;
use sov_solana_offchain_auth::capabilities::{
    SolanaOffchainAuthenticator, SolanaOffchainAuthenticatorInput, SolanaOffchainAuthenticatorTrait, 
};
use sov_solana_offchain_auth::authentication::SolanaOffchainSpecCompliantMessage;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::{BankConfig, Runtime as _};
use sov_test_utils::test_rollup::{GenesisSource, RollupBuilder, TestRollup};
use sov_test_utils::{generate_runtime, RtAgnosticBlueprint, TestSpec, TEST_DEFAULT_GAS_LIMIT, TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE};
use sov_value_setter::{CallMessage, ValueSetterConfig};
use tempfile::tempdir;

// Generate the test runtime with Solana offchain authenticator
generate_runtime! {
    name: TestRuntime,
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

impl<S: Spec> SolanaOffchainAuthenticatorTrait<S> for TestRuntime<S> {
    fn add_solana_offchain_auth(tx: RawTx) -> <Self::Auth as TransactionAuthenticator<S>>::Input {
        SolanaOffchainAuthenticatorInput::SolanaOffchain(tx)
    }
}

type RT = TestRuntime<TestSpec>;
type S = TestSpec;

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

fn create_mint_tx_json_bytes() -> Vec<u8> {
    let msg: TestRuntimeCall<S> = TestRuntimeCall::ValueSetter(CallMessage::SetValue { value: 1234, gas: None });
    let tx = UnsignedTransaction::<RT, S>::new(
        msg,
        config_value!("CHAIN_ID"),
        TEST_DEFAULT_MAX_PRIORITY_FEE,
        TEST_DEFAULT_MAX_FEE,
        UniquenessData::Generation(0),
        Some(TEST_DEFAULT_GAS_LIMIT.into()),
    );

    let tx_json_str = serde_json::to_string(&tx).unwrap();
    let tx_json_bytes = tx_json_str.as_bytes().to_vec();

    // Sanity check - since this JSON was used to create a ledger signature
    assert_eq!(tx_json_bytes.as_slice(), br#"{"runtime_call":{"value_setter":{"set_value":{"value":1234,"gas":null}}},"generation":0,"details":{"max_priority_fee_bips":0,"max_fee":"100000000000","gas_limit":[1000000000,1000000000],"chain_id":4321}}"#);

    tx_json_bytes
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
async fn test_submit_ledger_signed_transaction() {
    let rollup = create_test_rollup().await.expect("Failed to create rollup");

    let encoded_tx = create_mint_tx_json_bytes();

    // Data from a Ledger device
    let pubkey: [u8; 32] = bs58::decode("8YkzDTyLd3buhMw9CMfYYt3FLmcu1BeFr5nMeierYM1v").into_vec().unwrap().try_into().unwrap();
    let signature: Ed25519Signature = bs58::decode("2nZHcKfoYQMiWnQZWPoKE4q7xk1eJ6fwpt5T5QowzzD9ms6znCoCGcJS5t46csv9GAYpFQcVKsUeQWKhbnxUggvZ").into_vec().unwrap().as_slice().try_into().unwrap();

    let mut signed_message = make_preamble_for_message(&pubkey, encoded_tx.len() as u16).to_vec();
    signed_message.extend_from_slice(&encoded_tx);

    let message = SolanaOffchainSpecCompliantMessage::<S> {
        signed_message,
        signature
    };
    let raw_tx = RawTx::new(borsh::to_vec(&message).unwrap());
    println!("Raw tx bytes: {:?}", raw_tx.data);
    // let fully_baked_tx = RT::encode_with_solana_offchain_auth(raw_tx);
    // println!("Fully baked tx bytes: {:?}", fully_baked_tx.data);

    // TODO: add sequencer API to accept solana offchain txs
    let res = rollup.client.client.send_tx_to_sequencer(&raw_tx).await;
    println!("{res:?}");
    assert!(res.is_ok());

    // then assert the state change
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
