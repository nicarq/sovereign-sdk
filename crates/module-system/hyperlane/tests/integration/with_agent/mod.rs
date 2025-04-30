use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use helpers::{
    generate_setup, parse_eth_addr, setup_rollup, HyperlaneBuilder, ANVIL_ACCOUNTS,
    DEFAULT_FINALIZATION_BLOCKS, EVM_DOMAIN,
};
use preferred_sequencer_runtime::{TestRuntime, TestRuntimeCall};
use serde_json::{Map, Value};
use sov_api_spec::types::{self as api_types, GetSlotFilteredEventsResponse, IntOrHash};
use sov_api_spec::Client;
use sov_bank::Amount;
use sov_hyperlane_integration::{
    test_recipient, CallMessage, ExchangeRateAndGasPrice, HyperlaneAddress,
    InterchainGasPaymasterCallMessage, Ism,
};
use sov_modules_api::macros::config_value;
use sov_modules_api::{CryptoSpec, DispatchCall, HexHash, HexString, RawTx, Runtime, Spec};
use sov_test_utils::{default_test_signed_transaction, TestSpec, TestUser};
use tokio_stream::StreamExt;

use crate::igp::{default_gas_hashmap_to_safe_vec, oracle_data_hashmap_to_safe_vec};

mod configs;
mod helpers;
mod preferred_sequencer_runtime;

#[tokio::test(flavor = "multi_thread")]
async fn test_validator_announces_itself() {
    let dir = tempfile::tempdir().unwrap();
    let builder = HyperlaneBuilder::setup_image().await;
    let setup = generate_setup();
    let validator = setup.validators[0].clone();
    let rollup = setup_rollup(dir.path().to_path_buf(), setup).await;

    let mut hyperlane = builder
        .with_rollup_port(rollup.http_addr.port())
        .with_validators([&validator])
        .start()
        .await;

    // wait for first finalized block
    let mut slot_subscription = rollup.api_client.subscribe_slots().await.unwrap();
    for _ in 0..DEFAULT_FINALIZATION_BLOCKS {
        slot_subscription.next().await.unwrap().unwrap();
    }

    // look for `validator announce` event
    for slot in 0..DEFAULT_FINALIZATION_BLOCKS * 5 {
        slot_subscription.next().await.unwrap().unwrap();
        let events = rollup
            .api_client
            .get_slot_filtered_events(&IntOrHash::Integer(slot as u64), None)
            .await
            .unwrap();

        if let Some(process_event) = find_event(&events, "Mailbox/ValidatorAnnouncement") {
            assert_eq!(
                process_event["validator_announcement"]["address"],
                ANVIL_ACCOUNTS[1].0.to_string(),
            );
            assert!(process_event["validator_announcement"]["storage_location"]
                .as_str()
                .unwrap()
                .starts_with("file:///validator0/signatures"));

            rollup.shutdown().await.unwrap();
            return;
        }
    }

    rollup.shutdown().await.unwrap();
    hyperlane.print_stdout().await;
    panic!("Mailbox/ValidatorAnnouncement event not found");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_relayer_basic_dispatch_process() {
    let dir = tempfile::tempdir().unwrap();
    let builder = HyperlaneBuilder::setup_image().await;
    let setup = generate_setup();
    let relayer = setup.relayer.clone();
    let prover = setup.prover.clone();
    let prover_addr = prover.user_info.address();
    let rollup = setup_rollup(dir.path().to_path_buf(), setup).await;

    let mut hyperlane = builder
        .with_rollup_port(rollup.http_addr.port())
        .with_relayer(&relayer)
        .start()
        .await;

    // wait for first finalized block
    let mut slot_subscription = rollup.api_client.subscribe_slots().await.unwrap();
    for _ in 0..DEFAULT_FINALIZATION_BLOCKS {
        slot_subscription.next().await.unwrap().unwrap();
    }

    // set relayer igp config
    let relayer_config_tx = tx_set_relayer_config(&relayer);
    submit_tx(&rollup.api_client, relayer_config_tx).await;

    // register prover as a recipient
    let register_call = TestRuntimeCall::TestRecipient(test_recipient::CallMessage::Register {
        address: prover_addr.to_sender(),
        ism: Ism::AlwaysTrust,
    });
    let register_tx = encode_call(prover.user_info.private_key(), &register_call);
    submit_tx(&rollup.api_client, register_tx).await;

    // dispatch message to prover
    let dispatch_tx = tx_send_message(&relayer, prover_addr.to_sender(), None, b"Hello there");
    submit_tx(&rollup.api_client, dispatch_tx).await;

    // look for `process` event
    for slot in 0..DEFAULT_FINALIZATION_BLOCKS * 5 {
        slot_subscription.next().await.unwrap().unwrap();
        let events = rollup
            .api_client
            .get_slot_filtered_events(&IntOrHash::Integer(slot as u64), None)
            .await
            .unwrap();

        if let Some(process_event) = find_event(&events, "Mailbox/Process") {
            assert_eq!(
                process_event["process"]["recipient_address"],
                prover_addr.to_sender().to_string(),
            );
            assert_eq!(
                process_event["process"]["sender_address"],
                relayer.address().to_sender().to_string(),
            );

            let test_recipient_event =
                find_event(&events, "TestRecipient/MessageReceivedGeneric").unwrap();
            assert_eq!(
                test_recipient_event["MessageReceivedGeneric"]["body"],
                format!("0x{}", hex::encode("Hello there"))
            );

            rollup.shutdown().await.unwrap();
            return;
        }
    }

    rollup.shutdown().await.unwrap();
    hyperlane.print_stdout().await;
    panic!("Mailbox/Process event not found");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_multisig_ism() {
    let dir = tempfile::tempdir().unwrap();
    let builder = HyperlaneBuilder::setup_image().await;
    let setup = generate_setup();
    let relayer = setup.relayer.clone();
    let prover = setup.prover.clone();
    let prover_addr = prover.user_info.address();
    let validators = setup.validators.clone();
    let rollup = setup_rollup(dir.path().to_path_buf(), setup).await;

    let mut hyperlane = builder
        .with_rollup_port(rollup.http_addr.port())
        .with_relayer(&relayer)
        .with_validators(&validators[..2])
        .start()
        .await;

    // wait for first finalized block
    let mut slot_subscription = rollup.api_client.subscribe_slots().await.unwrap();
    for _ in 0..DEFAULT_FINALIZATION_BLOCKS {
        slot_subscription.next().await.unwrap().unwrap();
    }

    // set relayer igp config
    let relayer_config_tx = tx_set_relayer_config(&relayer);
    submit_tx(&rollup.api_client, relayer_config_tx).await;

    // register prover as a recipient with first 3 validators addresses for multisig
    let val_addresses: Vec<_> = ANVIL_ACCOUNTS[1..4]
        .iter()
        .map(|(addr, _)| addr.parse().unwrap())
        .collect();
    let register_call = TestRuntimeCall::TestRecipient(test_recipient::CallMessage::Register {
        address: prover_addr.to_sender(),
        ism: Ism::MessageIdMultisig {
            validators: val_addresses.try_into().unwrap(),
            threshold: 2,
        },
    });
    let register_tx = encode_call(prover.user_info.private_key(), &register_call);
    submit_tx(&rollup.api_client, register_tx).await;

    // dispatch message to prover
    let dispatch_tx = tx_send_message(&relayer, prover_addr.to_sender(), None, b"Hello there");
    submit_tx(&rollup.api_client, dispatch_tx).await;

    // look for `process` event
    // we use much bigger number of slots there to avoid flakiness
    // because we start 2 additional validator processes while rollup
    // is already producing slots, so transactions end up in later slots
    for slot in 0..DEFAULT_FINALIZATION_BLOCKS * 15 {
        slot_subscription.next().await.unwrap().unwrap();
        let events = rollup
            .api_client
            .get_slot_filtered_events(&IntOrHash::Integer(slot as u64), None)
            .await
            .unwrap();

        if let Some(process_event) = find_event(&events, "Mailbox/Process") {
            assert_eq!(
                process_event["process"]["recipient_address"],
                prover_addr.to_sender().to_string(),
            );
            assert_eq!(
                process_event["process"]["sender_address"],
                relayer.address().to_sender().to_string(),
            );

            rollup.shutdown().await.unwrap();
            return;
        }
    }

    rollup.shutdown().await.unwrap();
    hyperlane.print_stdout().await;
    panic!("Mailbox/Process event not found");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_process_message_from_evm_counterparty() {
    let dir = tempfile::tempdir().unwrap();
    let builder = HyperlaneBuilder::setup_image().await;
    let setup = generate_setup();
    let relayer = setup.relayer.clone();
    let prover = setup.prover.clone();
    let prover_addr = prover.user_info.address();
    let rollup = setup_rollup(dir.path().to_path_buf(), setup).await;

    let mut hyperlane = builder
        .with_rollup_port(rollup.http_addr.port())
        .with_relayer(&relayer)
        .with_evm_counterparty(prover_addr.to_sender())
        .start()
        .await;

    // wait for first finalized block
    let mut slot_subscription = rollup.api_client.subscribe_slots().await.unwrap();
    for _ in 0..DEFAULT_FINALIZATION_BLOCKS {
        slot_subscription.next().await.unwrap().unwrap();
    }

    // register prover as a recipient
    let register_call = TestRuntimeCall::TestRecipient(test_recipient::CallMessage::Register {
        address: prover_addr.to_sender(),
        ism: Ism::AlwaysTrust,
    });
    let register_tx = encode_call(prover.user_info.private_key(), &register_call);
    submit_tx(&rollup.api_client, register_tx).await;

    // dispatch test message to prover from evm
    let (expected_message, expected_message_id) = hyperlane.dispatch_msg_from_counterparty().await;
    let sender_addr = parse_eth_addr(ANVIL_ACCOUNTS[0].0);

    assert_eq!(expected_message.origin_domain, EVM_DOMAIN);
    assert_eq!(
        expected_message.dest_domain,
        config_value!("HYPERLANE_BRIDGE_DOMAIN")
    );
    assert_eq!(expected_message.sender, sender_addr);
    assert_eq!(expected_message.recipient, prover_addr.to_sender());

    // finalize the block with dispatched message
    hyperlane.mine_next_block_on_counterparty().await;

    // look for `process` event
    for slot in 0..DEFAULT_FINALIZATION_BLOCKS * 30 {
        slot_subscription.next().await.unwrap().unwrap();
        let events = rollup
            .api_client
            .get_slot_filtered_events(&IntOrHash::Integer(slot as u64), None)
            .await
            .unwrap();

        if let Some(process_event) = find_event(&events, "Mailbox/Process") {
            assert_eq!(
                process_event["process"]["recipient_address"],
                prover_addr.to_sender().to_string(),
            );
            assert_eq!(
                process_event["process"]["sender_address"],
                sender_addr.to_string(),
            );

            let test_recipient_event =
                find_event(&events, "TestRecipient/MessageReceivedGeneric").unwrap();
            assert_eq!(
                test_recipient_event["MessageReceivedGeneric"]["body"],
                expected_message.body.to_string(),
            );

            let process_id_event = find_event(&events, "Mailbox/ProcessId").unwrap();
            assert_eq!(
                process_id_event["process_id"]["id"],
                expected_message_id.to_string()
            );

            rollup.shutdown().await.unwrap();
            return;
        }
    }

    rollup.shutdown().await.unwrap();
    hyperlane.print_stdout().await;
    panic!("Mailbox/Process event not found");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_dispatch_message_to_evm_counterparty() {
    let dir = tempfile::tempdir().unwrap();
    let builder = HyperlaneBuilder::setup_image().await;
    let setup = generate_setup();
    let relayer = setup.relayer.clone();
    let prover = setup.prover.clone();
    let prover_addr = prover.user_info.address();
    let rollup = setup_rollup(dir.path().to_path_buf(), setup).await;

    let mut hyperlane = builder
        .with_rollup_port(rollup.http_addr.port())
        .with_relayer(&relayer)
        .with_evm_counterparty(prover_addr.to_sender())
        .start()
        .await;

    // wait for first finalized block
    let mut slot_subscription = rollup.api_client.subscribe_slots().await.unwrap();
    for _ in 0..DEFAULT_FINALIZATION_BLOCKS {
        slot_subscription.next().await.unwrap().unwrap();
    }

    // set relayer igp config
    let relayer_config_tx = tx_set_relayer_config(&relayer);
    submit_tx(&rollup.api_client, relayer_config_tx).await;

    let evm_recipient = hyperlane.evm_recipient.unwrap();
    // dispatch message to evm test recipient
    let dispatch_tx = tx_send_message(&relayer, evm_recipient, Some(EVM_DOMAIN), b"Hello there");
    submit_tx(&rollup.api_client, dispatch_tx).await;

    // look for `dispatch` event
    for slot in 0..DEFAULT_FINALIZATION_BLOCKS * 10 {
        slot_subscription.next().await.unwrap().unwrap();
        let events = rollup
            .api_client
            .get_slot_filtered_events(&IntOrHash::Integer(slot as u64), None)
            .await
            .unwrap();

        if let Some(process_event) = find_event(&events, "Mailbox/Dispatch") {
            assert_eq!(process_event["dispatch"]["destination_domain"], EVM_DOMAIN,);
            assert_eq!(
                process_event["dispatch"]["recipient_address"],
                evm_recipient.to_string(),
            );

            let message_id_event = find_event(&events, "Mailbox/DispatchId").unwrap();
            let message_id = message_id_event["dispatch_id"]["id"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();

            // Find the dispatched message on counterparty
            let evm_event = hyperlane.latest_message_on_counterparty().await;
            assert_eq!(
                evm_event.origin_domain,
                config_value!("HYPERLANE_BRIDGE_DOMAIN")
            );
            assert_eq!(evm_event.sender_address, relayer.address().to_sender());
            assert_eq!(evm_event.recipient_address, evm_recipient);
            assert_eq!(evm_event.id, message_id);

            rollup.shutdown().await.unwrap();
            return;
        }
    }

    rollup.shutdown().await.unwrap();
    hyperlane.print_stdout().await;
    panic!("Mailbox/Dispatch event not found");
}

fn tx_send_message(
    relayer: &TestUser<TestSpec>,
    recipient_address: HexHash,
    domain: Option<u32>,
    message_body: &[u8],
) -> RawTx {
    let call = TestRuntimeCall::Mailbox(CallMessage::Dispatch {
        domain: domain.unwrap_or(config_value!("HYPERLANE_BRIDGE_DOMAIN")),
        recipient: recipient_address,
        body: HexString(message_body.to_vec().try_into().unwrap()),
        metadata: None,
        relayer: Some(relayer.address()),
        gas_payment_limit: Amount::MAX,
    });

    encode_call(relayer.private_key(), &call)
}

fn encode_call(
    key: &<<TestSpec as Spec>::CryptoSpec as CryptoSpec>::PrivateKey,
    call_message: &<TestRuntime<TestSpec> as DispatchCall>::Decodable,
) -> RawTx {
    let tx = default_test_signed_transaction::<TestRuntime<TestSpec>, TestSpec>(
        key,
        call_message,
        nonce(),
        &<TestRuntime<TestSpec> as Runtime<TestSpec>>::CHAIN_HASH,
    );

    RawTx::new(borsh::to_vec(&tx).unwrap())
}

async fn submit_tx(client: &Client, tx_body: RawTx) {
    client
        .accept_tx(&api_types::AcceptTxBody {
            body: BASE64_STANDARD.encode(&tx_body),
        })
        .await
        .unwrap();
}

fn find_event(events: &GetSlotFilteredEventsResponse, event: &str) -> Option<Map<String, Value>> {
    events
        .data
        .iter()
        .find(|ev| ev.key == event)
        .map(|ev| ev.value.clone())
}

fn tx_set_relayer_config(relayer: &TestUser<TestSpec>) -> RawTx {
    let default_gas = Amount(2000);
    let domain_oracles = HashMap::from([
        (
            config_value!("HYPERLANE_BRIDGE_DOMAIN"),
            ExchangeRateAndGasPrice {
                gas_price: Amount(1),
                token_exchange_rate: 1,
            },
        ),
        (
            EVM_DOMAIN,
            ExchangeRateAndGasPrice {
                gas_price: Amount(1),
                token_exchange_rate: 1,
            },
        ),
    ]);
    let domain_gas = HashMap::from([
        (config_value!("HYPERLANE_BRIDGE_DOMAIN"), default_gas),
        (EVM_DOMAIN, default_gas),
    ]);
    let call = TestRuntimeCall::InterchainGasPaymaster(
        InterchainGasPaymasterCallMessage::SetRelayerConfig {
            domain_oracle_data: oracle_data_hashmap_to_safe_vec(domain_oracles),
            domain_default_gas: default_gas_hashmap_to_safe_vec(domain_gas),
            default_gas,
            beneficiary: Some(relayer.address()),
        },
    );

    encode_call(relayer.private_key(), &call)
}

fn nonce() -> u64 {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    NONCE.fetch_add(1, Ordering::Relaxed)
}
