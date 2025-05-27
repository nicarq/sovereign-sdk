use std::collections::HashSet;
use std::ops::Range;
use std::time::Duration;

use demo_stf::runtime::{self, Runtime};
use futures::StreamExt;
use rand::Rng;
use sov_address::MultiAddressEvm;
use sov_celestia_adapter::verifier::CelestiaSpec;
use sov_cli::wallet_state::PrivateKeyAndAddress;
use sov_cli::NodeClient;
use sov_modules_api::configurable_spec::ConfigurableSpec;
use sov_modules_api::execution_mode::Native;
use sov_modules_api::transaction::{Transaction, UnsignedTransaction};
use sov_modules_api::Spec;
use sov_modules_macros::config_value;
use sov_risc0_adapter::{Risc0, Risc0CryptoSpec};
use sov_rollup_interface::zk::CryptoSpec;
use sov_state::{DefaultStorageSpec, ProverStorage};
use sov_test_utils::{TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE};

use crate::test_helpers::CHAIN_HASH;

type NativeStorage = ProverStorage<DefaultStorageSpec<<Risc0CryptoSpec as CryptoSpec>::Hasher>>;

type S = ConfigurableSpec<
    CelestiaSpec,
    Risc0,
    Risc0,
    Risc0CryptoSpec,
    MultiAddressEvm,
    Native,
    NativeStorage,
>;

fn generate_dynamic_random_vectors(len_range: Range<usize>) -> Vec<Vec<u8>> {
    let mut rng = rand::thread_rng();
    let mut result = Vec::new();
    for length in len_range {
        let number_of_vectors = rng.gen_range(1..=3);

        let mut vectors_for_this_length = HashSet::new();

        while vectors_for_this_length.len() < number_of_vectors {
            let new_vector = (0..length).map(|_| rng.gen::<u8>()).collect::<Vec<u8>>();
            vectors_for_this_length.insert(new_vector);
        }

        result.extend(vectors_for_this_length.into_iter());
    }

    result
}

fn generate_call_message<S: Spec>(len_range: Range<usize>) -> Vec<runtime::RuntimeCall<S>>
where
    <S as sov_modules_api::Spec>::Address: sov_address::FromVmAddress<sov_address::EthereumAddress>,
{
    let payloads = generate_dynamic_random_vectors(len_range);
    let mut messages = Vec::with_capacity(payloads.len());

    for payload in payloads {
        messages.push(runtime::RuntimeCall::ValueSetter(
            sov_value_setter::CallMessage::SetManyValues(payload),
        ));
    }

    messages
}

async fn submit_blobs_increasing_size() -> anyhow::Result<()> {
    // Purpose of this test to check that celestia adapter can process batches of various sizes.
    // This test submits batches in range of size, sequentially.
    // To minimize potential compression related issues,
    // each payload is generated randomly, and for each length there are 3 payloads
    //
    // This test requires appropriate rollup running on port 12345
    let blobs_payload_bytes_range = 1..10000;
    let token_deployer_data =
        std::fs::read_to_string("../test-data/keys/token_deployer_private_key.json")
            .expect("Unable to read file to string");

    let token_deployer: PrivateKeyAndAddress<S> = serde_json::from_str(&token_deployer_data)
        .unwrap_or_else(|_| {
            panic!(
                "Unable to convert data {} to PrivateKeyAndAddress",
                &token_deployer_data
            )
        });

    let chain_id = config_value!("CHAIN_ID");
    let max_priority_fee_bips = TEST_DEFAULT_MAX_PRIORITY_FEE;
    let max_fee = TEST_DEFAULT_MAX_FEE;

    let messages = generate_call_message::<S>(blobs_payload_bytes_range);
    println!("Generate {} messages", messages.len());

    let rest_port = 12346;
    let client = NodeClient::new_at_localhost(rest_port).await?;

    let mut slot_subscription = client.client.subscribe_slots().await?;

    for (idx, message) in messages.into_iter().enumerate() {
        println!("Nonce {} . Going to submit message: {:?}", idx, message);
        let tx = Transaction::<Runtime<S>, S>::new_signed_tx(
            &token_deployer.private_key,
            &CHAIN_HASH,
            UnsignedTransaction::new(
                message,
                chain_id,
                max_priority_fee_bips,
                max_fee,
                idx as u64,
                None,
            ),
        );

        let _ = client.client.send_tx_to_sequencer(&tx).await;
        let slot = slot_subscription.next().await.unwrap()?;
        println!("SLOT: {} received", slot.number);
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "Run manually"]
async fn test_celestia_increasing_blob_sizes() -> anyhow::Result<()> {
    // cargo test -p sov-demo-rollup --test all_tests celestia::test_celestia_increasing_blob_sizes -- --nocapture --ignored
    submit_blobs_increasing_size().await
}
