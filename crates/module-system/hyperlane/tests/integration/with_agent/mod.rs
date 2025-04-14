use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use helpers::{generate_setup, setup_rollup, Hyperlane, DEFAULT_FINALIZATION_BLOCKS};
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
async fn test_relayer_basic_dispatch_process() {
    let dir = tempfile::tempdir().unwrap();
    let port = 22222;
    let mut hyperlane = Hyperlane::new().await;
    let setup = generate_setup();
    let relayer = setup.relayer.clone();
    let prover = setup.prover.clone();
    let prover_addr = prover.user_info.address();
    let rollup = setup_rollup(dir.path().to_path_buf(), port, setup).await;

    hyperlane.start(&relayer.private_key, port).await;

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
