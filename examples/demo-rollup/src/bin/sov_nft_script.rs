use std::time::Duration;

use clap::Parser;
use demo_stf::runtime::{Runtime, RuntimeCall};
use sov_cli::NodeClient;
use sov_demo_rollup::initialize_logging;
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
use sov_test_harness::{get_gas_funding_txs, AccountPool, AccountPoolConfig};

const DEFAULT_MAX_FEE: u64 = 1_000_000;
const DEFAULT_MAX_PRIORITY_FEE: PriorityFeeBips = PriorityFeeBips::from_percentage(0);
type DemoSpec = sov_modules_api::default_spec::DefaultSpec<MockZkVerifier, MockZkVerifier, Native>;
type DemoCryptoSpec = <DemoSpec as Spec>::CryptoSpec;
type DemoPrivateKey = <DemoCryptoSpec as CryptoSpec>::PrivateKey;
type DemoDaSpec = MockDaSpec;
type DemoRuntime = Runtime<DemoSpec, DemoDaSpec>;

const COLLECTION_1: &str = "Sovereign Squirrel Syndicate";
const COLLECTION_2: &str = "Celestial Dolphins";
const COLLECTION_3: &str = "Risky Rhinos";

const DUMMY_URL: &str = "http://foobar.storage";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to private keys file.
    #[arg(short, long)]
    private_keys_dir: String,

    /// REST API URL of the demo rollup.
    #[arg(short, long, default_value = "http://127.0.0.1:12346")]
    node_url: String,
}

pub fn build_transaction(
    signer: &DemoPrivateKey,
    message: CallMessage<DemoSpec>,
    nonce: u64,
) -> Transaction<DemoSpec> {
    tracing::info!(call_message = ?message, "Building transaction from CallMessage");
    let runtime_encoded_message = RuntimeCall::<DemoSpec, DemoDaSpec>::Nft(message);
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

fn build_create_collection_transactions(
    creator_pk: &DemoPrivateKey,
    start_nonce: &mut u64,
    base_uri: &str,
    collections: &[&str],
) -> Vec<Transaction<DemoSpec>> {
    tracing::info!(
        creator = %creator_pk.to_address::<<DemoSpec as Spec>::Address>(),
        collection_names = ?collections,
        base_uri,
        "Building create collections transactions"
    );
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

fn build_mint_transactions(
    creator_pk: &DemoPrivateKey,
    start_nonce: &mut u64,
    collection: &str,
    start_nft_id: &mut u64,
    num: usize,
    base_uri: &str,
    owner_pk: &DemoPrivateKey,
) -> Vec<Transaction<DemoSpec>> {
    let creator = creator_pk.to_address::<<DemoSpec as Spec>::Address>();
    let owner = owner_pk.to_address::<<DemoSpec as Spec>::Address>();
    tracing::info!(
        collection,
        creator = %creator,
        owner = %owner,
        "Building mint transactions"
    );

    (0..num)
        .map(|_| {
            let tx = build_transaction(
                creator_pk,
                get_mint_nft_message(&creator, collection, *start_nft_id, base_uri, &owner),
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
    initialize_logging();
    tracing::info!("Starting NFT demo script");
    let args = Args::parse();

    let account_pool_config =
        AccountPoolConfig::new(args.private_keys_dir.to_string(), args.node_url.clone(), 3);

    let node_client = NodeClient::new(&args.node_url)?;

    let account_pool = AccountPool::<DemoSpec>::new_from_config(account_pool_config).await?;

    let gas_funding_txs = get_gas_funding_txs::<DemoSpec>(&args.node_url, &account_pool).await?;

    let gas_transactions: Vec<_> = gas_funding_txs
        .into_iter()
        .map(|prepared_call_message| {
            let chain_id = config_value!("CHAIN_ID");
            borsh::to_vec(
                &account_pool
                    .build_transaction::<sov_bank::Bank<DemoSpec>, DemoRuntime, DemoDaSpec>(
                        prepared_call_message,
                        chain_id,
                        DEFAULT_MAX_PRIORITY_FEE,
                    ),
            )
            .expect("Failed to serialize_transaction")
        })
        .collect();

    tracing::info!(tx_num = %gas_transactions.len(), "Publishing gas funding txs...");
    node_client.publish_batch(gas_transactions, true).await?;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let first_index = account_pool.get_first_index_with_zero_nonce().unwrap();
    let creator_pk = account_pool
        .get_by_index(&first_index)
        .unwrap()
        .private_key();
    let owner_1_pk = account_pool
        .get_by_index(&(first_index + 1))
        .unwrap()
        .private_key();
    let owner_2_pk = account_pool
        .get_by_index(&(first_index + 2))
        .unwrap()
        .private_key();

    let mut nonce = 0;
    let collections = [COLLECTION_1, COLLECTION_2, COLLECTION_3];
    let transactions: Vec<_> =
        build_create_collection_transactions(creator_pk, &mut nonce, DUMMY_URL, &collections)
            .into_iter()
            .map(|tx| borsh::to_vec(&tx).expect("Failed to serialize transaction"))
            .collect();

    tracing::info!(tx_num = %transactions.len(), "Publishing collections creation transactions...");
    node_client.publish_batch(transactions, true).await?;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let mut nft_id = 1;
    let mut transactions = build_mint_transactions(
        creator_pk,
        &mut nonce,
        COLLECTION_1,
        &mut nft_id,
        15,
        DUMMY_URL,
        owner_1_pk,
    );

    transactions.extend(build_mint_transactions(
        creator_pk,
        &mut nonce,
        COLLECTION_1,
        &mut nft_id,
        5,
        DUMMY_URL,
        owner_2_pk,
    ));
    let mut nft_id = 1;
    transactions.extend(build_mint_transactions(
        creator_pk,
        &mut nonce,
        COLLECTION_2,
        &mut nft_id,
        20,
        DUMMY_URL,
        owner_1_pk,
    ));

    let transactions: Vec<_> = transactions
        .into_iter()
        .map(|tx| borsh::to_vec(&tx).expect("Failed to serialize transaction"))
        .collect();

    tracing::info!(tx_num = %transactions.len(), "Publishing NFT minting transactions...");
    node_client.publish_batch(transactions, true).await?;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let collection_1_address = get_collection_id::<DemoSpec>(
        COLLECTION_1,
        creator_pk
            .to_address::<<DemoSpec as Spec>::Address>()
            .as_ref(),
    );

    let mut owner_1_nonce = 0;
    let nft_ids_to_transfer: Vec<u64> = (1..=6).collect();
    let transactions = build_transfer_transactions(
        owner_1_pk,
        &mut owner_1_nonce,
        &collection_1_address,
        nft_ids_to_transfer,
    )
    .into_iter()
    .map(|tx| borsh::to_vec(&tx).expect("Failed to serialize transaction"))
    .collect();

    node_client.publish_batch(transactions, true).await?;

    println!("All NFT operations are completed!");

    Ok(())
}
