use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use helpers::{
    generate_setup, setup_rollup, Hyperlane, ANVIL_ACCOUNTS, DEFAULT_FINALIZATION_BLOCKS,
};
use preferred_sequencer_runtime::{TestRuntime, TestRuntimeCall};
use sov_api_spec::types::{self as api_types, IntOrHash};
use sov_api_spec::Client;
use sov_hyperlane_integration::{test_recipient, CallMessage, HyperlaneAddress, Ism};
use sov_modules_api::macros::config_value;
use sov_modules_api::{CryptoSpec, DispatchCall, HexHash, HexString, RawTx, Runtime, Spec};
use sov_test_utils::{default_test_signed_transaction, TestSpec};
use tokio_stream::StreamExt;

mod helpers;
mod preferred_sequencer_runtime;

#[tokio::test(flavor = "multi_thread")]
async fn test_validator_announces_itself() {
    let dir = tempfile::tempdir().unwrap();
    let mut hyperlane = Hyperlane::new().await;
    let setup = generate_setup();
    let relayer = setup.relayer.clone();
    let validator = setup.validators[0].clone();
    let rollup = setup_rollup(dir.path().to_path_buf(), setup).await;

    hyperlane
        .start(&relayer.private_key, rollup.http_addr.port())
        .await;

    hyperlane.start_validator(&validator.private_key).await;

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

        if let Some(process_event) = events
            .data
            .iter()
            .find(|ev| ev.key == "Mailbox/ValidatorAnnouncement")
        {
            assert_eq!(
                process_event.value["validator_announcement"]["address"],
                ANVIL_ACCOUNTS[1].0.to_string(),
            );
            assert!(
                process_event.value["validator_announcement"]["storage_location"]
                    .as_str()
                    .unwrap()
                    .starts_with("file:///validator0/signatures")
            );

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
    let mut hyperlane = Hyperlane::new().await;
    let setup = generate_setup();
    let relayer = setup.relayer.clone();
    let prover = setup.prover.clone();
    let prover_addr = prover.user_info.address();
    let rollup = setup_rollup(dir.path().to_path_buf(), setup).await;

    hyperlane
        .start(&relayer.private_key, rollup.http_addr.port())
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
    let register_tx = encode_call(prover.user_info.private_key(), 0, &register_call);
    submit_tx(&rollup.api_client, register_tx).await;

    // dispatch message to prover
    let dispatch_tx = tx_send_message(
        relayer.private_key(),
        prover_addr.to_sender(),
        b"Hello there",
        1,
    );
    submit_tx(&rollup.api_client, dispatch_tx).await;

    // look for `process` event
    for slot in 0..DEFAULT_FINALIZATION_BLOCKS * 5 {
        slot_subscription.next().await.unwrap().unwrap();
        let events = rollup
            .api_client
            .get_slot_filtered_events(&IntOrHash::Integer(slot as u64), None)
            .await
            .unwrap();

        if let Some(process_event) = events.data.iter().find(|ev| ev.key == "Mailbox/Process") {
            assert_eq!(
                process_event.value["process"]["recipient_address"],
                prover_addr.to_sender().to_string(),
            );
            assert_eq!(
                process_event.value["process"]["sender_address"],
                relayer.address().to_sender().to_string(),
            );

            let test_recipient_event = events
                .data
                .iter()
                .find(|ev| ev.key == "TestRecipient/MessageReceivedGeneric")
                .unwrap();
            assert_eq!(
                test_recipient_event.value["MessageReceivedGeneric"]["body"],
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
    let mut hyperlane = Hyperlane::new().await;
    let setup = generate_setup();
    let relayer = setup.relayer.clone();
    let prover = setup.prover.clone();
    let prover_addr = prover.user_info.address();
    let validators = setup.validators.clone();
    let rollup = setup_rollup(dir.path().to_path_buf(), setup).await;

    hyperlane
        .start(&relayer.private_key, rollup.http_addr.port())
        .await;

    // start only first two validators, more isn't needed due to threshold
    for validator in &validators[..2] {
        hyperlane.start_validator(&validator.private_key).await;
    }

    // wait for first finalized block
    let mut slot_subscription = rollup.api_client.subscribe_slots().await.unwrap();
    for _ in 0..DEFAULT_FINALIZATION_BLOCKS {
        slot_subscription.next().await.unwrap().unwrap();
    }

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
    let register_tx = encode_call(prover.user_info.private_key(), 0, &register_call);
    submit_tx(&rollup.api_client, register_tx).await;

    // dispatch message to prover
    let dispatch_tx = tx_send_message(
        relayer.private_key(),
        prover_addr.to_sender(),
        b"Hello there",
        1,
    );
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

        if let Some(process_event) = events.data.iter().find(|ev| ev.key == "Mailbox/Process") {
            assert_eq!(
                process_event.value["process"]["recipient_address"],
                prover_addr.to_sender().to_string(),
            );
            assert_eq!(
                process_event.value["process"]["sender_address"],
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

fn tx_send_message(
    relayer: &<<TestSpec as Spec>::CryptoSpec as CryptoSpec>::PrivateKey,
    recipient_address: HexHash,
    message_body: &[u8],
    nonce: u64,
) -> RawTx {
    let call = TestRuntimeCall::Mailbox(CallMessage::Dispatch {
        domain: config_value!("HYPERLANE_BRIDGE_DOMAIN"),
        recipient: recipient_address,
        body: HexString(message_body.to_vec().try_into().unwrap()),
        metadata: None,
    });

    encode_call(relayer, nonce, &call)
}

fn encode_call(
    key: &<<TestSpec as Spec>::CryptoSpec as CryptoSpec>::PrivateKey,
    nonce: u64,
    call_message: &<TestRuntime<TestSpec> as DispatchCall>::Decodable,
) -> RawTx {
    let tx = default_test_signed_transaction::<TestRuntime<TestSpec>, TestSpec>(
        key,
        call_message,
        nonce,
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
