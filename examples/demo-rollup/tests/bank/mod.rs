use std::net::SocketAddr;

use borsh::BorshSerialize;
use demo_stf::genesis_config::GenesisPaths;
use demo_stf::runtime::RuntimeCall;
use jsonrpsee::core::client::{Subscription, SubscriptionClientT};
use jsonrpsee::http_client::HttpClient;
use jsonrpsee::rpc_params;
use serde_json::{from_value, Value};
use sov_bank::event::Event as BankEvent;
use sov_bank::Coins;
use sov_ledger_rpc::client::RpcClient;
use sov_mock_da::{MockAddress, MockDaConfig, MockDaSpec};
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{Address, PrivateKey, Spec};
use sov_modules_stf_blueprint::kernels::basic::BasicKernelGenesisPaths;
use sov_sequencer::utils::SimpleClient;
use sov_stf_runner::RollupProverConfig;
use sov_test_utils::{TestPrivateKey, TestSpec};

use crate::test_helpers::start_rollup;

const TOKEN_SALT: u64 = 0;
const TOKEN_NAME: &str = "test_token";

#[tokio::test]
async fn bank_tx_tests_instant_finality() -> Result<(), anyhow::Error> {
    bank_tx_tests(0).await
}

#[tokio::test]
async fn bank_tx_tests_non_instant_finality() -> Result<(), anyhow::Error> {
    bank_tx_tests(3).await
}

async fn bank_tx_tests(finalization_blocks: u32) -> anyhow::Result<()> {
    let (port_tx, port_rx) = tokio::sync::oneshot::channel();

    let rollup_task = tokio::spawn(async move {
        start_rollup(
            port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Skip,
            MockDaConfig {
                // This value is important and should match ../test-data/genesis/integration-tests /sequencer_registry.json
                // Otherwise batches are going to be rejected
                sender_address: MockAddress::new([0; 32]),
                finalization_blocks,
                wait_attempts: 10,
            },
        )
        .await;
    });

    let port = port_rx.await.unwrap();

    // If the rollup throws an error, return it and stop trying to send the transaction
    tokio::select! {
        err = rollup_task => err?,
        res = send_test_bank_txs(port) => res?,
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
    let gas_tip = 0;
    let gas_limit = 0;
    let max_gas_price = None;
    Transaction::<TestSpec>::new_signed_tx(
        key,
        msg.try_to_vec().unwrap(),
        chain_id,
        gas_tip,
        gas_limit,
        max_gas_price,
        nonce,
    )
}

fn build_transfer_token_tx(
    key: &TestPrivateKey,
    token_address: Address,
    recipient: <TestSpec as Spec>::Address,
    amount: u64,
    nonce: u64,
) -> Transaction<TestSpec> {
    let msg =
        RuntimeCall::<TestSpec, MockDaSpec>::bank(sov_bank::CallMessage::<TestSpec>::Transfer {
            to: recipient,
            coins: Coins {
                amount,
                token_address,
            },
        });
    let chain_id = 0;
    let gas_tip = 0;
    let gas_limit = 0;
    let max_gas_price = None;
    Transaction::<TestSpec>::new_signed_tx(
        key,
        msg.try_to_vec().unwrap(),
        chain_id,
        gas_tip,
        gas_limit,
        max_gas_price,
        nonce,
    )
}

fn build_multiple_transfers(
    amounts: &[u64],
    signer_key: &TestPrivateKey,
    token_address: Address,
    recipient: <TestSpec as Spec>::Address,
    start_nonce: u64,
) -> Vec<Transaction<TestSpec>> {
    let mut txs = vec![];
    let mut nonce = start_nonce;
    for amt in amounts {
        txs.push(build_transfer_token_tx(
            signer_key,
            token_address,
            recipient,
            *amt,
            nonce,
        ));
        nonce += 1;
    }
    txs
}

async fn send_transactions_and_wait_slot(
    client: &SimpleClient,
    transactions: Vec<Transaction<TestSpec>>,
) -> Result<(), anyhow::Error> {
    let mut slot_subscription: Subscription<u64> = client
        .ws()
        .subscribe(
            "ledger_subscribeSlots",
            rpc_params![],
            "ledger_unsubscribeSlots",
        )
        .await?;

    client.send_transactions(transactions, None).await?;

    let _ = slot_subscription.next().await;

    Ok(())
}

async fn assert_balance(
    client: &SimpleClient,
    assert_amount: u64,
    token_address: Address,
    user_address: <TestSpec as Spec>::Address,
    version: Option<u64>,
) -> Result<(), anyhow::Error> {
    let balance_response = sov_bank::BankRpcClient::<TestSpec>::balance_of(
        client.http(),
        version,
        user_address,
        token_address,
    )
    .await?;
    assert_eq!(balance_response.amount.unwrap_or_default(), assert_amount);
    Ok(())
}

async fn assert_bank_event(
    client: &SimpleClient,
    event_number: u64,
    expected_event: BankEvent<TestSpec>,
) -> Result<(), anyhow::Error> {
    let response_event = <HttpClient as RpcClient<String, String, String>>::get_event_by_number(
        client.http(),
        event_number,
    )
    .await?
    .unwrap();
    if let Value::Object(ref map) = response_event {
        // Ensure "bank" is present in response json
        assert_eq!(map.get("module_name").unwrap(), "bank");
        // Attempt to deserialize the "body" of the bank key in the response to the Event type
        let bank_event = from_value::<BankEvent<TestSpec>>(map.get("event_value").unwrap().clone())
            .expect("Unable to deserialize Bank event");
        // Ensure the event generated is a TokenCreated event with the correct token_address
        assert_eq!(bank_event, expected_event);
        assert_eq!(
            map.get("module_address").unwrap(),
            "sov1r5glamudyy9ysysfjkwu3wf9cjqs98e47tzc6pxuqlp48phqk36sthwg6h"
        );
    } else {
        panic!("Event from rpc not an object");
    }
    Ok(())
}

async fn send_test_bank_txs(rpc_address: SocketAddr) -> Result<(), anyhow::Error> {
    let port = rpc_address.port();
    let client = SimpleClient::new("localhost", port).await?;
    let key = TestPrivateKey::generate();
    let user_address: <TestSpec as Spec>::Address = key.to_address();

    let token_address =
        sov_bank::get_token_address::<TestSpec>(TOKEN_NAME, &user_address, TOKEN_SALT);

    let recipient_key = TestPrivateKey::generate();
    let recipient_address: <TestSpec as Spec>::Address = recipient_key.to_address();

    let token_address_response = sov_bank::BankRpcClient::<TestSpec>::token_address(
        client.http(),
        TOKEN_NAME.to_owned(),
        user_address,
        TOKEN_SALT,
    )
    .await?;

    assert_eq!(token_address, token_address_response);

    // create token. height 2
    let tx = build_create_token_tx(&key, 0);
    send_transactions_and_wait_slot(&client, vec![tx]).await?;
    assert_balance(&client, 1000, token_address, user_address, None).await?;

    // transfer 100 tokens. assert sender balance. height 3
    let tx = build_transfer_token_tx(&key, token_address, recipient_address, 100, 1);
    send_transactions_and_wait_slot(&client, vec![tx]).await?;
    assert_balance(&client, 900, token_address, user_address, None).await?;

    // transfer 200 tokens. assert sender balance. height 4
    let tx = build_transfer_token_tx(&key, token_address, recipient_address, 200, 2);
    send_transactions_and_wait_slot(&client, vec![tx]).await?;
    assert_balance(&client, 700, token_address, user_address, None).await?;

    // assert sender balance at height 2.
    assert_balance(&client, 1000, token_address, user_address, Some(2)).await?;

    // assert sender balance at height 3.
    assert_balance(&client, 900, token_address, user_address, Some(3)).await?;

    // assert sender balance at height 4.
    assert_balance(&client, 700, token_address, user_address, Some(4)).await?;

    // 10 transfers of 10,11..20
    let transfer_amounts: Vec<u64> = (10u64..20).collect();
    let txs =
        build_multiple_transfers(&transfer_amounts, &key, token_address, recipient_address, 3);
    send_transactions_and_wait_slot(&client, txs).await?;

    assert_bank_event(&client, 0, BankEvent::TokenCreated { token_address }).await?;
    assert_bank_event(
        &client,
        1,
        BankEvent::TokenTransferred {
            token_address,
            amount: 100,
        },
    )
    .await?;
    assert_bank_event(
        &client,
        2,
        BankEvent::TokenTransferred {
            token_address,
            amount: 200,
        },
    )
    .await?;

    Ok(())
}
