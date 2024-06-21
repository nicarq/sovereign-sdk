use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use jsonrpsee::async_client::Client;
use jsonrpsee::core::client::{Subscription, SubscriptionClientT};
use jsonrpsee::rpc_params;
use jsonrpsee::ws_client::WsClientBuilder;
use sov_ledger_apis::rpc::client::RpcClient;
use sov_modules_api::prelude::tokio;
use sov_modules_stf_blueprint::{BatchSequencerOutcome, TxReceiptContents};
use sov_rollup_interface::rpc::{BatchResponse, ItemOrHash, QueryMode, SlotResponse, TxResponse};
use sov_rollup_interface::stf::TxEffect;
use tokio::task::JoinHandle;

use crate::args::Args;

pub fn start_slot_watcher_task(
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
