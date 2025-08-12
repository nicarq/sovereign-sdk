#![allow(unused_imports)]
use std::sync::Arc;
use sov_mock_zkvm::crypto::private_key::Ed25519PrivateKey;
use sov_node_client::NodeClient;
use tokio_stream::StreamExt;
use std::str::FromStr;

use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use sov_bank::{Amount, CallMessage as BankCallMessage, Coins, TokenId};
use sov_bank::BalanceResponse;
use sov_mock_da::{BlockProducingConfig, MockAddress, MockDaService};
use sov_mock_zkvm::crypto::Ed25519Signature;
use sov_mock_zkvm::MockZkvm;
use sov_modules_api::capabilities::{TransactionAuthenticator, UniquenessData};
use sov_modules_api::{prelude::*, PrivateKey, SafeVec};
use sov_modules_api::transaction::{Transaction, UnsignedTransaction};
use sov_modules_api::{FullyBakedTx, RawTx, Runtime, Spec};
use sov_modules_stf_blueprint::GenesisParams;
use sov_paymaster::{AuthorizedSequencers, PayeePolicy, PayerGenesisConfig, PaymasterConfig, PaymasterPolicyInitializer};
use sov_rollup_interface::execution_mode::Native;
use sov_sequencer::rest_api::AcceptTx;
use sov_solana_offchain_auth::utils::make_preamble_for_message;
use sov_solana_offchain_auth::capabilities::{
    SolanaOffchainAuthenticator, SolanaOffchainAuthenticatorInput, SolanaOffchainAuthenticatorTrait, 
};
use sov_solana_offchain_auth::authentication::{SolanaOffchainSimpleMessage, SolanaOffchainSpecCompliantMessage, SolanaOffchainUnsignedTransaction};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::{BankConfig, Runtime as _};
use sov_test_utils::test_rollup::{GenesisSource, RollupBuilder, TestRollup};
use sov_test_utils::{generate_runtime, RtAgnosticBlueprint, TestSpec, TestUser, TEST_DEFAULT_GAS_LIMIT, TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE};
use sov_value_setter::ValueSetterConfig;
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
    gas_enforcer: paymaster: sov_paymaster::Paymaster<S>,
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

async fn create_test_rollup() -> anyhow::Result<(
    TestRollup<SolanaOffchainAuthBlueprint<TestSpec, RT>>,
    TestUser<TestSpec>
)> {
    // Create genesis config
    let genesis_config =
        HighLevelOptimisticGenesisConfig::generate().add_accounts_with_default_balance(1);
    let sequencer = genesis_config.initial_sequencer.clone();
    let admin = genesis_config.additional_accounts()[0].clone();

    let rt_genesis_config = <RT as Runtime<TestSpec>>::GenesisConfig::from_minimal_config(
        genesis_config.clone().into(),
        ValueSetterConfig {
            admin: admin.address(),
        },
        PaymasterConfig {
            payers: [PayerGenesisConfig {
                payer_address: admin.address(),
                policy: PaymasterPolicyInitializer {
                    default_payee_policy: PayeePolicy::Allow {
                        max_fee: None,
                        gas_limit: None,
                        max_gas_price: None,
                        transaction_limit: None,
                    },
                    payees: SafeVec::new(),
                    authorized_sequencers: AuthorizedSequencers::All,
                    authorized_updaters: [admin.address()].as_ref().try_into().unwrap(),
                },
                sequencers_to_register: [sequencer.da_address].as_ref().try_into().unwrap(),
            }]
            .as_ref()
            .try_into()
            .unwrap(),
        },
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

    // Set up the rollup the usual way.
    let mut slot_subscription = rollup.api_client().subscribe_slots().await.unwrap();
    rollup
        .da_service
        .produce_n_blocks_now(5)
        .await
        .unwrap();
    for _ in 0..5 {
        let _ = slot_subscription.next().await.unwrap().unwrap();
    }

    Ok((rollup, admin))
}

fn create_transfer_tx_json(amount: Amount, recipient: &str) -> String {
    let msg: TestRuntimeCall<S> = TestRuntimeCall::Bank(BankCallMessage::Transfer {
        to: <S as Spec>::Address::from_str(
            recipient,
        )
        .unwrap(),
        coins: Coins {
            amount,
            // Use the gas token ID from the config (which is the pre-configured token)
            token_id: config_value!("GAS_TOKEN_ID"),
        },
    });
    let unsigned_tx = UnsignedTransaction::<RT, S>::new(
        msg,
        config_value!("CHAIN_ID"),
        TEST_DEFAULT_MAX_PRIORITY_FEE,
        TEST_DEFAULT_MAX_FEE,
        UniquenessData::Generation(0),
        Some(TEST_DEFAULT_GAS_LIMIT.into()),
    );
    let solana_unsigned_tx = SolanaOffchainUnsignedTransaction::<RT, S> {
        runtime_call: unsigned_tx.runtime_call,
        uniqueness: unsigned_tx.uniqueness,
        details: unsigned_tx.details,
        chain_hash: RT::CHAIN_HASH
    };

    serde_json::to_string(&solana_unsigned_tx).unwrap()
}

async fn submit_tx(client: &sov_api_spec::client::Client, raw_tx_bytes: Vec<u8>) -> reqwest::Response {
    let request = AcceptTx {
        body: sov_sequencer::rest_api::Base64Blob { blob: raw_tx_bytes },
    };
    
    let response = client.client()
        .post(format!("{}/sequencer/accept_solana_offchain_tx", client.baseurl()))
        .json(&request)
        .send()
        .await
        .expect("Failed to send request");

    response
}

async fn query_balance(client: &NodeClient, address: String) -> Option<Amount> {
    let gas_token_id: TokenId = config_value!("GAS_TOKEN_ID");
    
    // Query initial balance of recipient (should be 0)
    let response = client
        .query_rest_endpoint::<BalanceResponse>(&format!(
            "/modules/bank/tokens/{}/balances/{}", 
            gas_token_id, 
            address
        ))
        .await
        .expect("Failed to query balance");

    response.amount
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
    let tx_json_str = create_transfer_tx_json(Amount(10_000), "sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skqm7ehv");

    println!("{}", tx_json_str);
    // Sanity check - since this JSON was used to create a ledger signature
    assert_eq!(tx_json_str, r#"{"runtime_call":{"bank":{"transfer":{"to":"sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skqm7ehv","coins":{"amount":"10000","token_id":"token_1nyl0e0yweragfsatygt24zmd8jrr2vqtvdfptzjhxkguz2xxx3vs0y07u7"}}}},"uniqueness":{"generation":0},"details":{"max_priority_fee_bips":0,"max_fee":"100000000000","gas_limit":[1000000000,1000000000],"chain_id":4321},"chain_hash":"0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b"}"#);

    let encoded_tx = tx_json_str.as_bytes().to_vec();

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

    let (test_rollup, _admin) = create_test_rollup().await.expect("Failed to create rollup");
    let client = test_rollup.api_client();
    let _response = submit_tx(client, raw_tx_bytes).await;

    // TODO: re-sign from ledger
    // set up transfer to ledger's address
    // then transfer from ledger's address to some random address
    // assert state balances: that ledger has remaining balance and new address has the transferred
    // balance
}

#[tokio::test(flavor = "multi_thread")]
async fn test_submit_raw_signed_message_transaction() {
    let (test_rollup, admin) = create_test_rollup().await.expect("Failed to create rollup");
    
    let recipient_address = "sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skqm7ehv";
    let initial_amount = query_balance(&test_rollup.client, recipient_address.to_string()).await;
    assert_eq!(initial_amount, None, "Expected recipient to have no initial balance");

    let tx_str = create_transfer_tx_json(Amount(10_000), "sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skqm7ehv");
    let encoded_tx = tx_str.as_bytes().to_vec();
    let signer = admin.private_key();
    let pubkey = signer.pub_key();
    let signature = signer.sign(&encoded_tx);

    let message = SolanaOffchainSimpleMessage::<S> {
        signed_message: encoded_tx,
        pubkey,
        signature
    };
    let raw_tx_bytes = borsh::to_vec(&message).unwrap();

    let response = submit_tx(test_rollup.api_client(), raw_tx_bytes).await;
    
    assert!(response.status().is_success(), "Expected transaction to succeed");
    
    let final_balance = query_balance(&test_rollup.client, recipient_address.to_string()).await;
    assert_eq!(
        final_balance,
        Some(Amount::new(10_000)), 
        "Expected recipient to have received 10,000 tokens"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_submit_invalid_raw_signed_message_transaction() {
    let (test_rollup, admin) = create_test_rollup().await.expect("Failed to create rollup");

    let tx_str = create_transfer_tx_json(Amount(10_000), "sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skqm7ehv");
    let encoded_tx = tx_str.as_bytes().to_vec();
    let signer = admin.private_key();
    let pubkey = signer.pub_key();
    let mut signature_bytes = signer.sign(&encoded_tx).msg_sig.to_bytes();
    // mutate a random byte to make the signature invalid
    signature_bytes[5] = signature_bytes[5].wrapping_add(1);
    let signature: Ed25519Signature = signature_bytes.as_slice().try_into().unwrap();
    
    let message = SolanaOffchainSimpleMessage::<S> {
        signed_message: encoded_tx,
        pubkey,
        signature
    };
    let raw_tx_bytes = borsh::to_vec(&message).unwrap();

    let client = test_rollup.api_client();
    let response = submit_tx(client, raw_tx_bytes).await;
    
    assert_eq!(response.status(), 400, "Expected 400 status for invalid signature");
    let response_text = response.text().await.expect("Failed to read response body");
    
    assert!(response_text.contains("Signature verification failed") || 
            response_text.contains("Verification equation was not satisfied"),
            "Expected signature verification error, got: {}", response_text);
}

// Sanity check of the wrapper implementation
#[test]
fn test_auth_wrapper() {
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
