#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

mod account_pool;
mod args;
mod bank;

use std::env;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use borsh::ser::BorshSerialize;
use clap::Parser;
use demo_stf::authentication::ModAuth;
use demo_stf::runtime::Runtime;
use jsonrpsee::async_client::Client;
use jsonrpsee::core::client::{Subscription, SubscriptionClientT};
use jsonrpsee::http_client::HttpClientBuilder;
use jsonrpsee::rpc_params;
use jsonrpsee::ws_client::WsClientBuilder;
use serde::de::DeserializeOwned;
use sov_bank::Bank;
use sov_celestia_adapter::types::Namespace;
use sov_celestia_adapter::verifier::{CelestiaSpec, RollupParams};
use sov_celestia_adapter::{CelestiaConfig, CelestiaService};
use sov_ledger_apis::rpc::client::RpcClient;
use sov_mock_zkvm::MockZkVerifier;
use sov_modules_api::capabilities::Authenticator;
use sov_modules_api::default_spec::DefaultSpec;
use sov_modules_api::prelude::tokio;
use sov_modules_api::transaction::{PriorityFeeBips, Transaction, UnsignedTransaction};
use sov_modules_api::{EncodeCall, Module, Spec};
use sov_modules_stf_blueprint::{BatchSequencerOutcome, TxReceiptContents};
use sov_risc0_adapter::Risc0Verifier;
use sov_rollup_interface::da::BlockHeaderTrait;
use sov_rollup_interface::execution_mode::Native;
use sov_rollup_interface::rpc::{BatchResponse, ItemOrHash, QueryMode, SlotResponse, TxResponse};
use sov_rollup_interface::services::da::{DaService, SlotData};
use sov_rollup_interface::stf::{Batch, RawTx, TxEffect};
use tokio::task::JoinHandle;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

use crate::account_pool::AccountPool;
use crate::args::Args;
use crate::bank::generate_messages;

type ThisSpec = DefaultSpec<Risc0Verifier, MockZkVerifier, Native>;
/// Shortcut for module native authorization.
type Auth = ModAuth<ThisSpec, CelestiaSpec>;

/// The biggest blob in bytes we can submit to celestia
/// <https://celestiaorg.github.io/celestia-app/specs/params.html>
/// Our version is a little bit older, so this value differs.
/// Empirically taken from an error message.
// TODO: Will be used when blob size is going to be implemented.
#[allow(dead_code)]
const CELESTIA_MAX_TX_BYTES: u64 = 1973430;

/// Just DA part of full rollup_config.toml
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct HarnessConfig {
    da: CelestiaConfig,
}

fn from_toml_path<P: AsRef<Path>, R: DeserializeOwned>(path: P) -> anyhow::Result<R> {
    let mut contents = String::new();
    {
        let mut file = File::open(path)?;
        file.read_to_string(&mut contents)?;
    }

    let result: R = toml::from_str(&contents)?;

    Ok(result)
}

/// Combination of a module specific call message with expected sender.
pub struct PreparedCallMessage<S: Spec, M: Module<Spec = S>> {
    call_message: M::CallMessage,
    from: S::Address,
    max_fee: u64,
}

fn initialize_logging() {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(
            EnvFilter::from_str(&env::var("RUST_LOG").unwrap_or_else(|_| {
                "info,hyper=info,risc0_zkvm=warn,jmt=info,sov_celestia_adapter=info".to_string()
            }))
            .unwrap(),
        )
        .init();
}

fn prepare_bank_call_message(
    config: &Args,
    account_pool: &mut AccountPool<ThisSpec>,
    bank_message: PreparedCallMessage<ThisSpec, Bank<ThisSpec>>,
) -> RawTx {
    let PreparedCallMessage {
        call_message,
        from,
        max_fee,
    } = bank_message;
    tracing::debug!(?call_message, %from, "Iterating over call message");
    let account = account_pool.get_mut_account(&from).unwrap();
    let nonce = account.nonce;
    let runtime_msg =
        <Runtime<ThisSpec, CelestiaSpec> as EncodeCall<Bank<ThisSpec>>>::encode_call(call_message);
    let unsigned_tx = UnsignedTransaction {
        runtime_msg,
        chain_id: config.chain_id,
        max_priority_fee_bips: PriorityFeeBips::from_percentage(config.priority_fee_percent),
        max_fee,
        nonce,
        gas_limit: None,
    };
    let tx = Transaction::<ThisSpec>::new_signed_tx(&account.private_key, unsigned_tx);
    let authed_tx = Auth::encode(tx.try_to_vec().unwrap()).unwrap();
    account.nonce += 1;
    authed_tx
}

async fn submit_transactions(da_service: &CelestiaService, txs: Vec<RawTx>) -> anyhow::Result<()> {
    let batch = Batch { txs };
    let batch_bytes = batch.try_to_vec().expect("Failed to serialize batch");
    let fee = da_service.estimate_fee(batch_bytes.len()).await.unwrap();
    let tx_hash = da_service.send_transaction(&batch_bytes, fee).await?;
    tracing::info!(%tx_hash, "Submitted Tx");
    Ok(())
}

fn start_slot_watcher_task(
    config: &Args,
    wait_till_slot: Arc<AtomicU64>,
) -> JoinHandle<(u64, u64)> {
    let ledger_ws_url = config.get_ws_url();
    tokio::spawn(async move {
        let mut successful_count = 0;
        let mut error_count = 0;
        let ledger_rpc_client = WsClientBuilder::default()
            .build(ledger_ws_url)
            .await
            .unwrap();
        tracing::info!("Starting slot watcher");

        let mut slot_subscription: Subscription<u64> = ledger_rpc_client
            .subscribe(
                "ledger_subscribeSlots",
                rpc_params![],
                "ledger_unsubscribeSlots",
            )
            .await
            .unwrap();
        loop {
            let slot_number = match slot_subscription.next().await.transpose() {
                Ok(slot) => slot.unwrap_or_default(),
                Err(e) => {
                    tracing::info!(error = ?e, "Error during next slot subscription, resubscribing");
                    slot_subscription = ledger_rpc_client
                        .subscribe(
                            "ledger_subscribeSlots",
                            rpc_params![],
                            "ledger_unsubscribeSlots",
                        )
                        .await
                        .unwrap();
                    continue;
                }
            };
            tracing::info!(slot = ?slot_number, "Received processed slot");
            let wait_till = wait_till_slot.load(Ordering::Relaxed);
            tracing::info!(final_slot_number = wait_till, "Going to wait till");

            let slot = <Client as RpcClient<
                SlotResponse<BatchSequencerOutcome, TxReceiptContents>,
                BatchResponse<BatchSequencerOutcome, TxReceiptContents>,
                TxResponse<TxReceiptContents>,
            >>::get_slot_by_number::<'_, '_>(
                &ledger_rpc_client, slot_number, QueryMode::Full
            )
            .await
            .unwrap()
            .unwrap();

            let batches = slot.batches.unwrap_or_default();
            tracing::debug!(
                hash = hex::encode(slot.hash),
                batches = batches.len(),
                "Inspecting slot"
            );

            for batch in batches {
                let batch = match batch {
                    ItemOrHash::Hash(_) => {
                        panic!("asked for full batch, got hash");
                    }
                    ItemOrHash::Full(batch_response) => batch_response,
                };
                let txs = batch.txs.unwrap_or_default();
                tracing::debug!(txs = txs.len(), "Inspecting batch");
                for tx_response in txs {
                    let tx_response = match tx_response {
                        ItemOrHash::Hash(_) => {
                            panic!("asked for full tx, got hash");
                        }
                        ItemOrHash::Full(tx) => tx,
                    };
                    match tx_response.receipt {
                        TxEffect::Skipped(_) | TxEffect::Reverted(_) => {
                            error_count += 1;
                        }
                        TxEffect::Successful(_) => {
                            successful_count += 1;
                        }
                    }
                }
            }

            // Check if the slot is smaller than the value in `wait_till_slot`
            if slot_number >= wait_till {
                tracing::info!(slot = ?slot_number, "Slot is now greater than or equal to wait_till_slot. Exiting loop.");
                break;
            }
        }
        (successful_count, error_count)
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    initialize_logging();
    let config = Args::parse();

    // Setting up account pool
    let mut account_pool = AccountPool::<ThisSpec>::from_keys_in_folder(&config.private_keys_dir)?;
    if account_pool.is_empty() {
        anyhow::bail!("Cannot proceed without any known key");
    }
    for addr in account_pool.addresses() {
        tracing::debug!(address = %addr, "Address has been read from disk");
    }
    let client = HttpClientBuilder::default().build(&config.rpc_url)?;
    // Refreshing nonces before generating new users to avoid non needed RPC calls.
    account_pool.refresh_nonces(&client).await?;
    (0..config.new_users_count).for_each(|_| account_pool.generate_new_key());

    // Configuring DA service
    let harness_config: HarnessConfig = from_toml_path(&config.rollup_config_path).unwrap();
    tracing::info!(?harness_config, "Config of the target celestia node");
    let batch_namespace = config.get_namespace()?;
    tracing::debug!(?batch_namespace, derived_from=%config.celestia_batch_namespace, "Going to use batch namespace");
    let da_service = CelestiaService::new(
        harness_config.da,
        RollupParams {
            rollup_batch_namespace: batch_namespace,
            // We don't need proof namespace for this iteration
            rollup_proof_namespace: Namespace::MAX_PRIMARY_RESERVED,
        },
    )
    .await;
    let ledger_ws_url = config.get_ws_url();
    let ledger_rpc_client = WsClientBuilder::default()
        .build(ledger_ws_url.clone())
        .await
        .unwrap();

    // Calculating slot diff
    let first_head_header = da_service.get_head_block_header().await?;
    let head: Option<_> = <Client as RpcClient<
        SlotResponse<BatchSequencerOutcome, TxReceiptContents>,
        BatchResponse<BatchSequencerOutcome, TxReceiptContents>,
        TxResponse<TxReceiptContents>,
    >>::get_head::<'_, '_>(&ledger_rpc_client, QueryMode::Compact)
    .await
    .unwrap();
    let head_slot = head.unwrap();
    let slot_diff = first_head_header.header().height() - head_slot.number;
    tracing::info!(slot_diff, "Difference between DA height and Rollup slot");

    // Starting slot watcher
    let wait_till_slot = Arc::new(AtomicU64::from(u64::MAX));
    let slot_watcher = start_slot_watcher_task(&config, wait_till_slot.clone());

    let mut total_transactions_sent = 0;
    let txs_per_batch = config.max_batch_size_tx as usize;

    // Starting setup
    let bank_setup_batches =
        bank::setup::<ThisSpec>(&account_pool, &config.genesis_dir, &client, txs_per_batch).await?;
    for bank_setup_batch in bank_setup_batches {
        let setup_batch: Vec<_> = bank_setup_batch
            .into_iter()
            .map(|bank_message| prepare_bank_call_message(&config, &mut account_pool, bank_message))
            .collect();
        total_transactions_sent += setup_batch.len();
        submit_transactions(&da_service, setup_batch).await?;
    }

    // Sending actual transactions
    let generated_bank_messages: Vec<_> =
        generate_messages::<ThisSpec>(&account_pool, config.bank_transactions_count)?;
    let total_bank_messages = generated_bank_messages.len();
    let mut raw_txs: Vec<RawTx> = Vec::with_capacity(txs_per_batch);
    for (idx, bank_message) in generated_bank_messages.into_iter().enumerate() {
        let authed_tx = prepare_bank_call_message(&config, &mut account_pool, bank_message);
        raw_txs.push(authed_tx);
        // Checking if batch needs to be published.
        // Doing it here, so bytes of current tx can be estimated.
        let tx_count_current = raw_txs.len();
        // TODO: Check bytes here
        if (tx_count_current > 0 && tx_count_current % txs_per_batch == 0)
            || idx == (total_bank_messages - 1)
        {
            total_transactions_sent += tx_count_current;
            let batch_txs = std::mem::take(&mut raw_txs);
            submit_transactions(&da_service, batch_txs).await?;
        }
    }

    // Final checks
    let last_head_header = da_service.get_head_block_header().await?;
    tracing::info!(header = %last_head_header.header().display(), "DA header after submission is completed");
    // Adding 2 just for error correction
    let last_submitted_slot = last_head_header.header().height() - slot_diff + 2;
    wait_till_slot.store(last_submitted_slot, Ordering::Relaxed);
    let (successful_count, error_count) = slot_watcher.await?;
    tracing::info!(
        total_transactions_sent,
        successful = successful_count,
        error = error_count,
        "All transactions has been submitted and should've been processed by now"
    );
    Ok(())
}
