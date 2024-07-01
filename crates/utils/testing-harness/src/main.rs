#![doc = include_str!("../README.md")]

mod account_pool;
mod args;
mod bank;
mod constants;
mod harness_config;
mod logging;
mod slot_watcher;
mod submit_transactions;
mod types;
mod utils;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use clap::Parser;
use jsonrpsee::http_client::HttpClientBuilder;
use sov_celestia_adapter::types::Namespace;
use sov_celestia_adapter::verifier::RollupParams;
use sov_celestia_adapter::CelestiaService;
use sov_modules_api::prelude::tokio;
use sov_rollup_interface::da::BlockHeaderTrait;
use sov_rollup_interface::services::da::{DaService, SlotData};
use sov_rollup_interface::stf::RawTx;

use crate::account_pool::AccountPool;
use crate::args::Args;
use crate::bank::{generate_bank_transfer_messages, generate_token_contract_creation_messages};
use crate::harness_config::HarnessConfig;
use crate::logging::initialize_logging;
use crate::slot_watcher::start_slot_watcher_task;
use crate::submit_transactions::{prepare_message_for_submission, submit_transactions};
use crate::types::ThisSpec;

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
    let client = HttpClientBuilder::default().build(&config.rest_url)?;
    // Refreshing nonces before generating new users to avoid non needed RPC calls.
    account_pool.refresh_nonces(&client).await?;
    (0..config.new_users_count).for_each(|_| account_pool.generate_new_key());

    // Configuring DA service
    let harness_config = HarnessConfig::from_toml_path(&config.rollup_config_path).unwrap();
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

    let ledger_client = sov_ledger_json_client::Client::new(&format!("{}/ledger", config.rest_url));

    // Calculating slot diff
    let first_head_header = da_service.get_head_block_header().await?;
    let head_slot = ledger_client.get_latest_slot(None).await?;
    let slot_diff = first_head_header.header().height() - head_slot.data.number;
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
            .map(|bank_message| {
                prepare_message_for_submission(&config, &mut account_pool, bank_message)
            })
            .collect();
        total_transactions_sent += setup_batch.len();
        submit_transactions(&da_service, setup_batch).await?;
    }

    // Sending actual transactions
    let mut all_messages = vec![];

    let bank_transfer_messages: Vec<_> =
        generate_bank_transfer_messages::<ThisSpec>(&account_pool, config.bank_transactions_count)?;
    let token_creation_messages = generate_token_contract_creation_messages(
        &account_pool,
        config.token_contracts_count.unwrap_or_default(),
    )?;

    all_messages.extend(bank_transfer_messages);
    all_messages.extend(token_creation_messages);

    let mut raw_txs: Vec<RawTx> = Vec::with_capacity(txs_per_batch);

    let total_num_messages = all_messages.len();

    for (idx, message) in all_messages.into_iter().enumerate() {
        let authed_tx = prepare_message_for_submission(&config, &mut account_pool, message);
        // Checking if batch needs to be published.
        // Doing it here, so bytes of current tx can be estimated.
        let tx_count_current = raw_txs.len();
        // TODO: Check bytes here
        if (tx_count_current > 0 && tx_count_current % txs_per_batch == 0)
            || idx == (total_num_messages - 1)
        {
            total_transactions_sent += tx_count_current;
            let batch_txs = std::mem::take(&mut raw_txs);
            submit_transactions(&da_service, batch_txs).await?;
        }

        raw_txs.push(authed_tx);
    }

    if !raw_txs.is_empty() {
        total_transactions_sent += raw_txs.len();
        let batch_txs = std::mem::take(&mut raw_txs);
        submit_transactions(&da_service, batch_txs).await?;
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
