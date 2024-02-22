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
use sov_modules_api::default_context::DefaultContext;
use sov_modules_api::default_signature::private_key::DefaultPrivateKey;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{Address, PrivateKey, Spec};
use sov_modules_stf_blueprint::kernels::basic::BasicKernelGenesisPaths;
use sov_rollup_interface::rpc::PaginatedEventResponse;
use sov_sequencer::utils::SimpleClient;
use sov_stf_runner::RollupProverConfig;

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

fn build_create_token_tx(key: &DefaultPrivateKey, nonce: u64) -> Transaction<DefaultContext> {
    let user_address: <DefaultContext as Spec>::Address = key.to_address();
    let msg = RuntimeCall::<DefaultContext, MockDaSpec>::bank(sov_bank::CallMessage::<
        DefaultContext,
    >::CreateToken {
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
    Transaction::<DefaultContext>::new_signed_tx(
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
    key: &DefaultPrivateKey,
    token_address: Address,
    recipient: <DefaultContext as Spec>::Address,
    amount: u64,
    nonce: u64,
) -> Transaction<DefaultContext> {
    let msg = RuntimeCall::<DefaultContext, MockDaSpec>::bank(sov_bank::CallMessage::<
        DefaultContext,
    >::Transfer {
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
    Transaction::<DefaultContext>::new_signed_tx(
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
    signer_key: &DefaultPrivateKey,
    token_address: Address,
    recipient: <DefaultContext as Spec>::Address,
    start_nonce: u64,
) -> Vec<Transaction<DefaultContext>> {
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
    transactions: Vec<Transaction<DefaultContext>>,
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
    user_address: <DefaultContext as Spec>::Address,
    version: Option<u64>,
) -> Result<(), anyhow::Error> {
    let balance_response = sov_bank::BankRpcClient::<DefaultContext>::balance_of(
        client.http(),
        version,
        user_address,
        token_address,
    )
    .await?;
    assert_eq!(balance_response.amount.unwrap_or_default(), assert_amount);
    Ok(())
}

async fn assert_bank_transfer_events_paged(
    client: &SimpleClient,
    token_address: Address,
    num_events_to_fetch: usize,
    total_num: usize,
    expected_transfer_list: Vec<u64>,
) -> Result<(), anyhow::Error> {
    let mut num_fetched = 0;
    let mut next_key = None;
    let mut expected_transfer_list = expected_transfer_list.clone();
    loop {
        let response_events = <HttpClient as RpcClient<String, String, String>>::get_events_by_key(
            client.http(),
            "token_transfer",
            Some("sov1r5glamudyy9ysysfjkwu3wf9cjqs98e47tzc6pxuqlp48phqk36sthwg6h"),
            None,
            num_events_to_fetch as u64,
            next_key.as_deref(),
        )
        .await?
        .unwrap();
        let paginated_response: PaginatedEventResponse =
            serde_json::from_value(response_events).unwrap();
        let _: Vec<_> = paginated_response
            .events_response
            .iter()
            .map(|e| {
                assert_eq!(e.module_name, "bank");
                assert_eq!(
                    e.module_address,
                    "sov1r5glamudyy9ysysfjkwu3wf9cjqs98e47tzc6pxuqlp48phqk36sthwg6h"
                );
                let bank_event = from_value::<BankEvent<DefaultContext>>(e.event_value.clone())
                    .expect("Unable to deserialize Bank event");
                assert_eq!(
                    bank_event,
                    BankEvent::TokenTransferred {
                        token_address,
                        amount: expected_transfer_list.remove(0)
                    }
                );
            })
            .collect();
        num_fetched += paginated_response.events_response.len();
        if num_fetched == total_num {
            assert!(paginated_response.next.is_none());
            break;
        } else {
            assert!(paginated_response.next.is_some());
            assert_eq!(
                paginated_response.events_response.len(),
                num_events_to_fetch
            );
            next_key = paginated_response.next;
        }
    }
    assert!(expected_transfer_list.is_empty());
    Ok(())
}

async fn assert_bank_module_events_paged(
    client: &SimpleClient,
    num_events_to_fetch: usize,
    total_num: usize,
    expected_enum_disc: Vec<std::mem::Discriminant<BankEvent<DefaultContext>>>,
) -> Result<(), anyhow::Error> {
    let mut num_fetched = 0;
    let mut next_key = None;
    let mut expected_enum_disc = expected_enum_disc.clone();
    loop {
        let response_events =
            <HttpClient as RpcClient<String, String, String>>::get_events_by_module_address(
                client.http(),
                "sov1r5glamudyy9ysysfjkwu3wf9cjqs98e47tzc6pxuqlp48phqk36sthwg6h",
                num_events_to_fetch as u64,
                next_key.as_deref(),
            )
            .await?
            .unwrap();
        let paginated_response: PaginatedEventResponse =
            serde_json::from_value(response_events).unwrap();
        let _: Vec<_> = paginated_response
            .events_response
            .iter()
            .map(|e| {
                assert_eq!(e.module_name, "bank");
                assert_eq!(
                    e.module_address,
                    "sov1r5glamudyy9ysysfjkwu3wf9cjqs98e47tzc6pxuqlp48phqk36sthwg6h"
                );
                let bank_event = std::mem::discriminant(
                    &from_value::<BankEvent<DefaultContext>>(e.event_value.clone())
                        .expect("Unable to deserialize Bank event"),
                );
                assert_eq!(bank_event, expected_enum_disc.remove(0));
            })
            .collect();
        num_fetched += paginated_response.events_response.len();
        if num_fetched == total_num {
            assert!(paginated_response.next.is_none());
            break;
        } else {
            assert!(paginated_response.next.is_some());
            assert_eq!(
                paginated_response.events_response.len(),
                num_events_to_fetch
            );
            next_key = paginated_response.next;
        }
    }
    assert!(expected_enum_disc.is_empty());
    Ok(())
}

async fn send_test_bank_txs(rpc_address: SocketAddr) -> Result<(), anyhow::Error> {
    let port = rpc_address.port();
    let client = SimpleClient::new("localhost", port).await?;
    let key = DefaultPrivateKey::generate();
    let user_address: <DefaultContext as Spec>::Address = key.to_address();

    let token_address =
        sov_bank::get_token_address::<DefaultContext>(TOKEN_NAME, &user_address, TOKEN_SALT);

    let recipient_key = DefaultPrivateKey::generate();
    let recipient_address: <DefaultContext as Spec>::Address = recipient_key.to_address();

    let token_address_response = sov_bank::BankRpcClient::<DefaultContext>::token_address(
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

    let response_event =
        <HttpClient as RpcClient<String, String, String>>::get_event_by_number(client.http(), 1)
            .await?
            .unwrap();
    let expected = serde_json::json!({
        "event_value": {
            "TokenCreated": {
                "token_address": token_address
            }
        },
        "module_address": "sov1r5glamudyy9ysysfjkwu3wf9cjqs98e47tzc6pxuqlp48phqk36sthwg6h",
        "module_name": "bank"
    });
    assert_eq!(response_event, expected);
    if let Value::Object(ref map) = response_event {
        // Ensure "bank" is present in response json
        assert_eq!(map.get("module_name").unwrap(), "bank");
        // Attempt to deserialize the "body" of the bank key in the response to the Event type
        let bank_event =
            from_value::<BankEvent<DefaultContext>>(map.get("event_value").unwrap().clone())
                .expect("Unable to deserialize Bank event");
        // Ensure the event generated is a TokenCreated event with the correct token_address
        assert_eq!(bank_event, BankEvent::TokenCreated { token_address });
        assert_eq!(
            map.get("module_address").unwrap(),
            "sov1r5glamudyy9ysysfjkwu3wf9cjqs98e47tzc6pxuqlp48phqk36sthwg6h"
        );
    } else {
        panic!("Event from rpc not an object");
    }

    // assert events for all transfers
    let mut transfer_amount_list = vec![100, 200];
    transfer_amount_list.extend(transfer_amounts);
    // 12 transfer events
    assert_bank_transfer_events_paged(&client, token_address, 7, 12, transfer_amount_list).await?;

    // assert getting events using module address
    let mut event_variants = vec![std::mem::discriminant(&BankEvent::TokenCreated {
        token_address,
    })];
    for _ in 0..12 {
        event_variants.push(std::mem::discriminant(&BankEvent::TokenTransferred {
            token_address,
            amount: 0,
        }))
    }
    // 13 events for bank module. 1 token create + 12 transfers
    assert_bank_module_events_paged(&client, 5, 13, event_variants).await?;

    Ok(())
}
