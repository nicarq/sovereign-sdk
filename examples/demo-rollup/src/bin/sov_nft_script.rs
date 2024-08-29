use std::time::Duration;

use demo_stf::runtime::RuntimeCall;
use sov_mock_da::MockDaSpec;
use sov_mock_zkvm::MockZkVerifier;
use sov_modules_api::macros::config_value;
use sov_modules_api::transaction::{PriorityFeeBips, Transaction, UnsignedTransaction};
use sov_modules_api::{PrivateKey, Spec};
use sov_nft::utils::{
    get_collection_id, get_create_collection_message, get_mint_nft_message,
    get_transfer_nft_message,
};
use sov_nft::{CallMessage, CollectionId};
use sov_rollup_interface::execution_mode::Native;
use sov_rollup_interface::zk::CryptoSpec;

const DEFAULT_MAX_FEE: u64 = 100_000_000;
const DEFAULT_MAX_PRIORITY_FEE: PriorityFeeBips = PriorityFeeBips::from_percentage(0);
type DemoSpec = sov_modules_api::default_spec::DefaultSpec<MockZkVerifier, MockZkVerifier, Native>;
type DemoCryptoSpec = <DemoSpec as Spec>::CryptoSpec;
type DemoPrivateKey = <DemoCryptoSpec as CryptoSpec>::PrivateKey;

const COLLECTION_1: &str = "Sovereign Squirrel Syndicate";
const COLLECTION_2: &str = "Celestial Dolphins";
const COLLECTION_3: &str = "Risky Rhinos";

const DUMMY_URL: &str = "http://foobar.storage";

pub fn build_transaction(
    signer: &DemoPrivateKey,
    message: CallMessage<DemoSpec>,
    nonce: u64,
) -> Transaction<DemoSpec> {
    let runtime_encoded_message = RuntimeCall::<DemoSpec, MockDaSpec>::Nft(message);
    let chain_id = config_value!("CHAIN_ID");
    let max_priority_fee_bips = DEFAULT_MAX_PRIORITY_FEE;
    let max_fee = DEFAULT_MAX_FEE;
    let gas_limit = None;

    Transaction::<DemoSpec>::new_signed_tx(
        signer,
        UnsignedTransaction::new(
            borsh::to_vec(&runtime_encoded_message).unwrap(),
            chain_id,
            max_priority_fee_bips,
            max_fee,
            nonce,
            gas_limit,
        ),
    )
}

pub fn build_create_collection_transactions(
    creator_pk: &DemoPrivateKey,
    start_nonce: &mut u64,
    base_uri: &str,
    collections: &[&str],
) -> Vec<Transaction<DemoSpec>> {
    collections
        .iter()
        .map(|&collection_name| {
            let tx = build_transaction(
                creator_pk,
                get_create_collection_message(
                    &creator_pk.to_address::<<DemoSpec as Spec>::Address>(),
                    collection_name,
                    base_uri,
                ),
                *start_nonce,
            );
            *start_nonce = start_nonce.wrapping_add(1);
            tx
        })
        .collect()
}

/// Convenience and readability wrapper for build_mint_nft_transaction
pub fn build_mint_transactions(
    creator_pk: &DemoPrivateKey,
    start_nonce: &mut u64,
    collection: &str,
    start_nft_id: &mut u64,
    num: usize,
    base_uri: &str,
    owner_pk: &DemoPrivateKey,
) -> Vec<Transaction<DemoSpec>> {
    (0..num)
        .map(|_| {
            let tx = build_transaction(
                creator_pk,
                get_mint_nft_message(
                    &creator_pk.to_address::<<DemoSpec as Spec>::Address>(),
                    collection,
                    *start_nft_id,
                    base_uri,
                    &owner_pk.to_address::<<DemoSpec as Spec>::Address>(),
                ),
                *start_nonce,
            );
            *start_nft_id = start_nft_id.wrapping_add(1);
            *start_nonce = start_nonce.wrapping_add(1);
            tx
        })
        .collect()
}

pub fn build_transfer_transactions(
    signer: &DemoPrivateKey,
    start_nonce: &mut u64,
    collection_id: &CollectionId,
    nft_ids: Vec<u64>,
) -> Vec<Transaction<DemoSpec>> {
    nft_ids
        .into_iter()
        .map(|nft_id| {
            let new_owner = DemoPrivateKey::generate().to_address::<<DemoSpec as Spec>::Address>();
            let tx = build_transaction(
                signer,
                get_transfer_nft_message(collection_id, nft_id, &new_owner),
                *start_nonce,
            );
            *start_nonce = start_nonce.wrapping_add(1);
            tx
        })
        .collect()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let creator_pk = DemoPrivateKey::generate();
    let owner_1_pk = DemoPrivateKey::generate();
    let owner_2_pk = DemoPrivateKey::generate();

    let rest_port = 12346;
    let sequencer_client =
        sov_sequencer_json_client::Client::new(&format!("http://127.0.0.1:{rest_port}/sequencer"));

    let mut nonce = 0;
    let collections = [COLLECTION_1, COLLECTION_2, COLLECTION_3];
    let transactions =
        build_create_collection_transactions(&creator_pk, &mut nonce, DUMMY_URL, &collections);

    sequencer_client
        .publish_batch_with_serialized_txs(&transactions)
        .await?;

    // sleep is necessary because of how the sequencer currently works
    // without the sleep, there is a concurrency issue and some transactions would be ignored
    // TODO: remove after https://github.com/Sovereign-Labs/sovereign-sdk/issues/949 is fixed
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let mut nft_id = 1;
    let mut transactions = build_mint_transactions(
        &creator_pk,
        &mut nonce,
        COLLECTION_1,
        &mut nft_id,
        15,
        DUMMY_URL,
        &owner_1_pk,
    );

    transactions.extend(build_mint_transactions(
        &creator_pk,
        &mut nonce,
        COLLECTION_1,
        &mut nft_id,
        5,
        DUMMY_URL,
        &owner_2_pk,
    ));
    let mut nft_id = 1;
    transactions.extend(build_mint_transactions(
        &creator_pk,
        &mut nonce,
        COLLECTION_2,
        &mut nft_id,
        20,
        DUMMY_URL,
        &owner_1_pk,
    ));

    sequencer_client
        .publish_batch_with_serialized_txs(&transactions)
        .await?;

    // TODO: remove after https://github.com/Sovereign-Labs/sovereign-sdk/issues/949 is fixed
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let collection_1_address = get_collection_id::<DemoSpec>(
        COLLECTION_1,
        creator_pk
            .to_address::<<DemoSpec as Spec>::Address>()
            .as_ref(),
    );

    let mut owner_1_nonce = 0;
    let nft_ids_to_transfer: Vec<u64> = (1..=6).collect();
    transactions = build_transfer_transactions(
        &owner_1_pk,
        &mut owner_1_nonce,
        &collection_1_address,
        nft_ids_to_transfer,
    );

    sequencer_client
        .publish_batch_with_serialized_txs(&transactions)
        .await?;

    Ok(())
}
