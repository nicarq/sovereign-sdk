#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

mod account_pool;
mod args;
mod constants;
mod ctrl_c_handler;
mod da_blob_sender;
mod harness_config;
mod module_message_generators;
mod prepared_call_messages;
mod slot_watcher;
mod utils;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub use account_pool::*;
use anyhow::Context;
use args::Args;
use clap::Parser;
use constants::DEFAULT_CHANNEL_SIZE;
use ctrl_c_handler::start_ctrl_c_handler;
use da_blob_sender::DaBlobSender;
use harness_config::HarnessConfig;
pub use module_message_generators::*;
pub use prepared_call_messages::*;
use slot_watcher::start_slot_watcher_task;
use sov_bank::Bank;
use sov_celestia_adapter::types::Namespace;
use sov_celestia_adapter::verifier::RollupParams;
use sov_celestia_adapter::CelestiaService;
use sov_modules_api::prelude::tokio;
use sov_modules_api::Spec;
use sov_modules_stf_blueprint::Runtime;
use sov_prover_incentives::ProverIncentives;
use sov_rollup_interface::node::da::{DaService, DaServiceWithRetries};
pub use utils::*;

/// Starting the actual harness.
pub async fn start<S, Da, R>() -> anyhow::Result<()>
where
    S: Spec,
    Da: DaService,
    R: Runtime<S, Da::Spec>
        + sov_modules_api::EncodeCall<Bank<S>>
        + sov_modules_api::EncodeCall<ProverIncentives<S, Da::Spec>>
        + 'static,
{
    let config = Args::parse();

    let account_pool_config = AccountPoolConfig::new(
        config.private_keys_dir.to_string(),
        config.node_url.clone(),
        config.new_users_count,
    );

    let account_pool = AccountPool::new_from_config(account_pool_config).await?;

    let should_stop = Arc::new(AtomicBool::new(false));

    let (serialized_messages_tx, serialized_messages_rx) =
        tokio::sync::mpsc::channel::<SerializedPreparedCallMessage>(DEFAULT_CHANNEL_SIZE);

    let harness_config =
        HarnessConfig::from_toml_path(&config.rollup_config_path).with_context(|| {
            format!(
                "failed to parse rollup config at {}",
                config.rollup_config_path
            )
        })?;
    tracing::debug!(config = ?harness_config, "HarnessConfig is parsed");

    let da_service = CelestiaService::new(
        harness_config.da,
        RollupParams {
            rollup_batch_namespace: config.get_rollup_batch_namespace()?,
            // We don't need proof namespace for this iteration
            rollup_proof_namespace: Namespace::MAX_PRIMARY_RESERVED,
        },
    )
    .await;

    let gas_funding_message_sender = get_gas_funding_message_sender::<S, Da::Spec, R>(
        &config.node_url,
        account_pool.clone(),
        serialized_messages_tx.clone(),
        should_stop.clone(),
    )
    .await?;
    tracing::debug!("Gas funding messages sender gas been initialized");

    let module_message_senders = get_message_senders::<S, Da::Spec, R>(
        should_stop.clone(),
        account_pool.clone(),
        serialized_messages_tx.clone(),
    )?;

    // NOTE: We send the funding messages first without waiting on an interval. We know this
    // iterator has a finite length.
    Box::new(gas_funding_message_sender)
        .send_messages(config.max_num_txs, None)
        .await;

    // Now we set the other senders running, these iterators may or may not end.
    for sender in module_message_senders.into_iter() {
        sender
            .send_messages(config.max_num_txs, config.interval)
            .await;
    }

    let da_blob_sender: DaBlobSender<S, R> = DaBlobSender::new(
        config.clone(),
        account_pool.clone(),
        DaServiceWithRetries::new_fast(da_service),
        serialized_messages_rx,
        should_stop.clone(),
    );

    let slot_watcher_handle = start_slot_watcher_task(&config, should_stop.clone());

    start_ctrl_c_handler(should_stop.clone(), serialized_messages_tx.clone());

    da_blob_sender.send_messages_to_da().await;

    // Collating results...
    tracing::info!("Collecting results...");
    let (successful_count, error_count) = slot_watcher_handle.await?;
    let total_observed = successful_count + error_count;

    if let Some(max_num_txs) = config.max_num_txs {
        if (total_observed as usize) < max_num_txs {
            tracing::warn!(
                observer = total_observed,
                sent = max_num_txs,
                "Observed less transactions that submitted"
            );
        } else {
            tracing::info!(
                observer = total_observed,
                sent = max_num_txs,
                "Potentially observed all transactions"
            );
        }
    }

    tracing::info!(
        successful = successful_count,
        error = error_count,
        total_expected = ?config.max_num_txs,
        "All transactions has been submitted and should've been processed by now"
    );

    Ok(())
}
