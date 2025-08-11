#![allow(unused_imports)]
use std::sync::Arc;
use sov_mock_zkvm::crypto::private_key::Ed25519PrivateKey;
use tokio_stream::StreamExt;
use std::str::FromStr;

use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use sov_bank::{Amount, Coins, TokenId};
use sov_mock_da::{BlockProducingConfig, MockAddress, MockDaService};
use sov_mock_zkvm::crypto::Ed25519Signature;
use sov_mock_zkvm::MockZkvm;
use sov_modules_api::capabilities::{TransactionAuthenticator, UniquenessData};
use sov_modules_api::{prelude::*, PrivateKey};
use sov_modules_api::transaction::{Transaction, UnsignedTransaction};
use sov_modules_api::{FullyBakedTx, RawTx, Runtime, Spec};
use sov_modules_stf_blueprint::GenesisParams;
use sov_paymaster::PaymasterConfig;
use sov_rollup_interface::execution_mode::Native;
use sov_sequencer::rest_api::AcceptTx;
use sov_solana_offchain_auth::utils::make_preamble_for_message;
use sov_solana_offchain_auth::capabilities::{
    SolanaOffchainAuthenticator, SolanaOffchainAuthenticatorInput, SolanaOffchainAuthenticatorTrait, 
};
use sov_solana_offchain_auth::authentication::{SolanaOffchainRawMessage, SolanaOffchainSpecCompliantMessage};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::{BankConfig, Runtime as _};
use sov_test_utils::test_rollup::{GenesisSource, RollupBuilder, TestRollup};
use sov_test_utils::{generate_runtime, RtAgnosticBlueprint, TestSpec, TEST_DEFAULT_GAS_LIMIT, TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE};
use sov_value_setter::{CallMessage, ValueSetterConfig};
use tempfile::tempdir;

mod blueprint;
use blueprint::SolanaOffchainAuthBlueprint;

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

async fn create_test_rollup() -> anyhow::Result<TestRollup<SolanaOffchainAuthBlueprint<TestSpec, RT>>> {
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
    let rollup = RollupBuilder::<SolanaOffchainAuthBlueprint<TestSpec, RT>>::new(
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

fn create_tx_json_bytes() -> Vec<u8> {
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
    assert_eq!(tx_json_str, r#"{"runtime_call":{"value_setter":{"set_value":{"value":1234,"gas":null}}},"uniqueness":{"generation":0},"details":{"max_priority_fee_bips":0,"max_fee":"100000000000","gas_limit":[1000000000,1000000000],"chain_id":4321}}"#);

    tx_json_bytes
}

async fn create_rollup_submit_tx_and_assert_state(raw_tx_bytes: Vec<u8>) {
    let test_rollup = create_test_rollup().await.expect("Failed to create rollup");

    // Set up the rollup the usual way.
    let mut slot_subscription = test_rollup.api_client().subscribe_slots().await.unwrap();
    test_rollup
        .da_service
        .produce_n_blocks_now(5)
        .await
        .unwrap();
    for _ in 0..5 {
        let _ = slot_subscription.next().await.unwrap().unwrap();
    }

    // Submit via the custom Solana offchain endpoint
    let client = test_rollup.api_client();
    let request = AcceptTx {
        body: sov_sequencer::rest_api::Base64Blob { blob: raw_tx_bytes },
    };
    
    let response = client.client()
        .post(format!("{}/sequencer/accept_solana_offchain_tx", client.baseurl()))
        .json(&request)
        .send()
        .await
        .expect("Failed to send request");

    assert!(response.status().is_success(), "Failed to submit transaction: {:?}", response.text().await);
    
    println!("Transaction submitted with response: {:?}", response);
    // assert_eq!(tx_response.status, "submitted");

    // Produce a block to include the transaction
    // rollup.produce_block().await.expect("Failed to produce block");
    
    // Verify the transaction was included
    // Note: You may want to add additional verification here to check the value was set correctly

    // then assert the state change
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
    let encoded_tx = create_tx_json_bytes();

    // Data from a Ledger device
    let pubkey: [u8; 32] = bs58::decode("8YkzDTyLd3buhMw9CMfYYt3FLmcu1BeFr5nMeierYM1v").into_vec().unwrap().try_into().unwrap();
    let signature: Ed25519Signature = bs58::decode("2nZHcKfoYQMiWnQZWPoKE4q7xk1eJ6fwpt5T5QowzzD9ms6znCoCGcJS5t46csv9GAYpFQcVKsUeQWKhbnxUggvZ").into_vec().unwrap().as_slice().try_into().unwrap();

    let mut signed_message = make_preamble_for_message(&pubkey, encoded_tx.len() as u16).to_vec();
    signed_message.extend_from_slice(&encoded_tx);

    let message = SolanaOffchainSpecCompliantMessage::<S> {
        signed_message,
        signature
    };
    let raw_tx_bytes = borsh::to_vec(&message).unwrap();

    create_rollup_submit_tx_and_assert_state(raw_tx_bytes).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_submit_raw_signed_message_transaction() {
    let encoded_tx = create_tx_json_bytes();

    let signer = Ed25519PrivateKey::generate();
    let pubkey = signer.pub_key();
    let signature = signer.sign(&encoded_tx);
    // let signature: Ed25519Signature = bs58::decode("2nZHcKfoYQMiWnQZWPoKE4q7xk1eJ6fwpt5T5QowzzD9ms6znCoCGcJS5t46csv9GAYpFQcVKsUeQWKhbnxUggvZ").into_vec().unwrap().as_slice().try_into().unwrap();

    let message = SolanaOffchainRawMessage::<S> {
        signed_message: encoded_tx,
        pubkey,
        signature
    };
    let raw_tx_bytes = borsh::to_vec(&message).unwrap();

    create_rollup_submit_tx_and_assert_state(raw_tx_bytes).await;
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
