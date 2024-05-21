use borsh::BorshSerialize;
use demo_stf::genesis_config::GenesisPaths;
use demo_stf::runtime::RuntimeCall;
use jsonrpsee::core::client::{Subscription, SubscriptionClientT};
use jsonrpsee::http_client::HttpClient;
use jsonrpsee::rpc_params;
use serde_json::{from_value, Value};
use sov_bank::event::Event as BankEvent;
use sov_bank::utils::TokenHolder;
use sov_bank::{Coins, TokenId};
use sov_kernels::basic::BasicKernelGenesisPaths;
use sov_ledger_apis::rpc::client::RpcClient;
use sov_mock_da::{MockAddress, MockDaConfig, MockDaSpec};
use sov_mock_zkvm::{MockCodeCommitment, MockZkVerifier};
use sov_modules_api::transaction::{PriorityFeeBips, Transaction, UnsignedTransaction};
use sov_modules_api::{PrivateKey, Spec};
use sov_rollup_interface::rpc::{AggregatedProofResponse, BatchResponse, SlotResponse, TxResponse};
use sov_rollup_interface::zk::aggregated_proof::{
    AggregateProofVerifier, AggregatedProofPublicData,
};
use sov_sequencer::utils::SimpleClient;
use sov_stf_runner::RollupProverConfig;
use sov_test_utils::{TestPrivateKey, TestSpec};

use crate::test_helpers::{get_appropriate_rollup_prover_config, read_private_keys, start_rollup};

const TOKEN_SALT: u64 = 0;
const TOKEN_NAME: &str = "test_token";
const MAX_TX_FEE: u64 = 10_000;

struct TestCase {
    wait_for_aggregated_proof: bool,
    finalization_blocks: u32,
}

#[tokio::test]
async fn bank_tx_tests_instant_finality() -> Result<(), anyhow::Error> {
    let test_case = TestCase {
        wait_for_aggregated_proof: true,
        finalization_blocks: 0,
    };
    let rollup_prover_config = get_appropriate_rollup_prover_config();
    bank_tx_tests(test_case, rollup_prover_config).await
}

#[tokio::test]
async fn bank_tx_tests_non_instant_finality() -> Result<(), anyhow::Error> {
    let test_case = TestCase {
        wait_for_aggregated_proof: false,
        finalization_blocks: 3,
    };
    bank_tx_tests(test_case, RollupProverConfig::Skip).await
}

async fn bank_tx_tests(
    test_case: TestCase,
    rollup_prover_config: RollupProverConfig,
) -> anyhow::Result<()> {
    let (port_tx, port_rx) = tokio::sync::oneshot::channel();

    let rollup_task = tokio::spawn(async move {
        start_rollup(
            port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            rollup_prover_config,
            MockDaConfig {
                // This value is important and should match ../test-data/genesis/integration-tests /sequencer_registry.json
                // Otherwise batches are going to be rejected
                sender_address: MockAddress::new([0; 32]),
                finalization_blocks: test_case.finalization_blocks,
                wait_attempts: 30,
            },
        )
        .await;
    });
    let port = port_rx.await.unwrap().port();
    let client = SimpleClient::new("localhost", port).await?;

    // If the rollup throws an error, return it and stop trying to send the transaction
    tokio::select! {
        err = rollup_task => err?,
        res = send_test_bank_txs(test_case, client) => res?,
    };
    Ok(())
}

fn build_create_token_tx(key: &TestPrivateKey, nonce: u64) -> Transaction<TestSpec> {
    let user_address: <TestSpec as Spec>::Address = key.to_address();
    let msg =
        RuntimeCall::<TestSpec, MockDaSpec>::bank(sov_bank::CallMessage::<TestSpec>::CreateToken {
            salt: TOKEN_SALT,
            token_name: TOKEN_NAME.to_string(),
            initial_balance: 1000,
            minter_address: user_address,
            authorized_minters: vec![],
        });
    let chain_id = 0;
    let max_priority_fee_bips = PriorityFeeBips::ZERO;
    let max_fee = MAX_TX_FEE;
    let gas_limit = None;
    Transaction::<TestSpec>::new_signed_tx(
        key,
        UnsignedTransaction::new(
            msg.try_to_vec().unwrap(),
            chain_id,
            max_priority_fee_bips,
            max_fee,
            nonce,
            gas_limit,
        ),
    )
}

fn build_transfer_token_tx(
    key: &TestPrivateKey,
    token_id: TokenId,
    recipient: <TestSpec as Spec>::Address,
    amount: u64,
    nonce: u64,
) -> Transaction<TestSpec> {
    let msg =
        RuntimeCall::<TestSpec, MockDaSpec>::bank(sov_bank::CallMessage::<TestSpec>::Transfer {
            to: recipient,
            coins: Coins { amount, token_id },
        });
    let chain_id = 0;
    let max_priority_fee_bips = PriorityFeeBips::ZERO;
    let max_fee = MAX_TX_FEE;
    let gas_limit = None;
    Transaction::<TestSpec>::new_signed_tx(
        key,
        UnsignedTransaction::new(
            msg.try_to_vec().unwrap(),
            chain_id,
            max_priority_fee_bips,
            max_fee,
            nonce,
            gas_limit,
        ),
    )
}

fn build_multiple_transfers(
    amounts: &[u64],
    signer_key: &TestPrivateKey,
    token_id: TokenId,
    recipient: <TestSpec as Spec>::Address,
    start_nonce: u64,
) -> Vec<Transaction<TestSpec>> {
    let mut txs = vec![];
    let mut nonce = start_nonce;
    for amt in amounts {
        txs.push(build_transfer_token_tx(
            signer_key, token_id, recipient, *amt, nonce,
        ));
        nonce += 1;
    }
    txs
}

async fn send_transactions_and_wait_slot(
    client: &SimpleClient,
    transactions: &[Transaction<TestSpec>],
) -> Result<(), anyhow::Error> {
    let mut slot_subscription: Subscription<u64> = client
        .ws()
        .subscribe(
            "ledger_subscribeSlots",
            rpc_params![],
            "ledger_unsubscribeSlots",
        )
        .await?;

    client.send_transactions(transactions).await?;

    let _ = slot_subscription.next().await;

    Ok(())
}

async fn subscribe_proof(
    client: &SimpleClient,
) -> Result<Subscription<AggregatedProofResponse>, anyhow::Error> {
    Ok(client
        .ws()
        .subscribe(
            "ledger_subscribeAggregatedProof",
            rpc_params![],
            "ledger_unsubscribeAggregatedProof",
        )
        .await?)
}

async fn assert_balance(
    client: &SimpleClient,
    assert_amount: u64,
    token_id: TokenId,
    user_address: <TestSpec as Spec>::Address,
    version: Option<u64>,
) -> Result<(), anyhow::Error> {
    let balance_response = sov_bank::BankRpcClient::<TestSpec>::balance_of(
        client.http(),
        version,
        user_address,
        token_id,
    )
    .await?;
    assert_eq!(balance_response.amount.unwrap_or_default(), assert_amount);
    Ok(())
}

async fn assert_aggregated_proof(
    initial_slot: u64,
    final_slot: u64,
    client: &SimpleClient,
) -> Result<(), anyhow::Error> {
    let proof_resp = RpcClient::<
        SlotResponse<u32, u32>,
        BatchResponse<u32, u32>,
        TxResponse<u32>,
    >::get_aggregated_proof(client.http())
    .await?
    .expect("Proof missing in the ledger db");

    let verifier = AggregateProofVerifier::<MockZkVerifier>::new(MockCodeCommitment::default());
    verifier.verify(&proof_resp.proof)?;

    let proof_pub_data = proof_resp.proof.public_data();
    // We test inequality because proofs are saved asynchronously in the db.
    assert!(initial_slot <= proof_pub_data.initial_slot_number);
    assert!(final_slot <= proof_pub_data.final_slot_number);

    let proof_data_info_resp = RpcClient::<
        SlotResponse<u32, u32>,
        BatchResponse<u32, u32>,
        TxResponse<u32>,
    >::get_aggregated_proof_info(client.http())
    .await?
    .expect("Proof missing in the ledger db");

    assert!(initial_slot <= proof_data_info_resp.initial_slot_number);
    assert!(final_slot <= proof_data_info_resp.final_slot_number);

    Ok(())
}

fn assert_aggregated_proof_public_data(
    initial_slot: u64,
    final_slot: u64,
    pub_data: &AggregatedProofPublicData,
) {
    assert_eq!(initial_slot, pub_data.initial_slot_number);
    assert_eq!(final_slot, pub_data.final_slot_number);
}

async fn assert_bank_event<S: Spec>(
    client: &SimpleClient,
    event_number: u64,
    expected_event: BankEvent<S>,
) -> Result<(), anyhow::Error> {
    let response_event = <HttpClient as RpcClient<String, String, String>>::get_event_by_number(
        client.http(),
        event_number,
    )
    .await?
    .unwrap();
    println!("{:?}", &response_event);
    if let Value::Object(ref map) = response_event {
        let event_value = map.get("event_value").unwrap();
        // Ensure "bank" is present in response json
        assert_eq!(map.get("module_name").unwrap(), "bank");
        // Attempt to deserialize the "body" of the bank key in the response to the Event type
        let bank_event = from_value::<BankEvent<S>>(event_value.clone())
            .expect("Unable to deserialize Bank event");
        // Ensure the event generated is a TokenCreated event with the correct token_id
        assert_eq!(bank_event, expected_event);
    } else {
        panic!("Event from rpc not an object");
    }
    Ok(())
}

async fn send_test_bank_txs(
    test_case: TestCase,
    client: SimpleClient,
) -> Result<(), anyhow::Error> {
    let key_and_address = read_private_keys::<TestSpec>("tx_signer_private_key.json");
    let key = key_and_address.private_key;
    let user_address: <TestSpec as Spec>::Address = key_and_address.address;

    let token_id = sov_bank::get_token_id::<TestSpec>(TOKEN_NAME, &user_address, TOKEN_SALT);

    let recipient_key = TestPrivateKey::generate();
    let recipient_address: <TestSpec as Spec>::Address = recipient_key.to_address();

    let token_id_response = sov_bank::BankRpcClient::<TestSpec>::token_id(
        client.http(),
        TOKEN_NAME.to_owned(),
        user_address,
        TOKEN_SALT,
    )
    .await?;

    let mut aggregated_proof_subscription = subscribe_proof(&client).await?;
    assert_eq!(token_id, token_id_response);

    // create token. height 2
    let tx = build_create_token_tx(&key, 0);
    send_transactions_and_wait_slot(&client, &[tx]).await?;
    assert_balance(&client, 1000, token_id, user_address, None).await?;

    // transfer 100 tokens. assert sender balance. height 3
    let tx = build_transfer_token_tx(&key, token_id, recipient_address, 100, 1);
    send_transactions_and_wait_slot(&client, &[tx]).await?;
    assert_balance(&client, 900, token_id, user_address, None).await?;

    // transfer 200 tokens. assert sender balance. height 4
    let tx = build_transfer_token_tx(&key, token_id, recipient_address, 200, 2);
    send_transactions_and_wait_slot(&client, &[tx]).await?;
    assert_balance(&client, 700, token_id, user_address, None).await?;

    // assert sender balance at height 2.
    assert_balance(&client, 1000, token_id, user_address, Some(2)).await?;

    // assert sender balance at height 3.
    assert_balance(&client, 900, token_id, user_address, Some(3)).await?;

    // assert sender balance at height 4.
    assert_balance(&client, 700, token_id, user_address, Some(4)).await?;

    // 10 transfers of 10,11..20
    let transfer_amounts: Vec<u64> = (10u64..20).collect();
    let txs = build_multiple_transfers(&transfer_amounts, &key, token_id, recipient_address, 3);
    send_transactions_and_wait_slot(&client, &txs).await?;

    assert_bank_event::<TestSpec>(
        &client,
        0,
        BankEvent::TokenCreated {
            token_name: TOKEN_NAME.to_owned(),
            coins: Coins {
                amount: 1000,
                token_id,
            },
            minter: TokenHolder::User(user_address),
            authorized_minters: vec![],
        },
    )
    .await?;
    assert_bank_event::<TestSpec>(
        &client,
        1,
        BankEvent::TokenTransferred {
            from: TokenHolder::User(user_address),
            to: TokenHolder::User(recipient_address),
            coins: Coins {
                amount: 100,
                token_id,
            },
        },
    )
    .await?;
    assert_bank_event::<TestSpec>(
        &client,
        2,
        BankEvent::TokenTransferred {
            from: TokenHolder::User(user_address),
            to: TokenHolder::User(recipient_address),
            coins: Coins {
                amount: 200,
                token_id,
            },
        },
    )
    .await?;

    if test_case.wait_for_aggregated_proof {
        let aggregated_proof_resp = aggregated_proof_subscription.next().await.unwrap()?;
        let pub_data = aggregated_proof_resp.proof.public_data();
        assert_aggregated_proof_public_data(1, 1, pub_data);
        assert_aggregated_proof(1, 1, &client).await?;
    }

    Ok(())
}
