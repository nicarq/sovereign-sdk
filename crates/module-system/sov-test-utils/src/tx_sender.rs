use std::marker::PhantomData;

use anyhow::Context;
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use reqwest::StatusCode;
use sov_api_spec::types::{IntOrHash, Slot};
use sov_cli::NodeClient;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{Runtime, Spec};

/// Submits transactions to the rollup, either directly or via sequencer.
#[async_trait]
pub trait TxSender<S: Spec, R: Runtime<S>> {
    /// Returns rollup_height of the batch has been produced.
    ///
    /// TODO(@neysofu): we should return a blob hash instead, because the rollup
    /// height can't actually be known for sure.
    async fn send_txs(
        &self,
        client: &NodeClient,
        transactions: &[Transaction<R, S>],
    ) -> anyhow::Result<u64>;
}

/// Submits transactions to the rollup through a sequencer.
#[derive(Default)]
pub struct SequencerTxSender<R: Runtime<S>, S: Spec> {
    phantom: PhantomData<(R, S)>,
}

#[async_trait]
impl<R: Runtime<S>, S: Spec> TxSender<S, R> for SequencerTxSender<R, S> {
    async fn send_txs(
        &self,
        client: &NodeClient,
        transactions: &[Transaction<R, S>],
    ) -> anyhow::Result<u64> {
        let slot_subscription = client
            .client
            .subscribe_slots()
            .await
            .context("Failed to subscribe to slots!")?;

        let _submitted_batch_info = client
            .client
            .publish_batch_with_serialized_txs(transactions)
            .await?;

        wait_for_batch_to_be_processed(slot_subscription, &client.client).await
    }
}

// Wait for the first non empty batch.
async fn wait_for_batch_to_be_processed(
    mut slot_subscription: BoxStream<'static, anyhow::Result<Slot>>,
    ledger_client: &sov_api_spec::Client,
) -> anyhow::Result<u64> {
    let wait_for = 1_000;
    for _ in 0..wait_for {
        let rollup_height = slot_subscription
            .next()
            .await
            .transpose()?
            .map(|slot| slot.number)
            .unwrap_or_default();

        let batch_response = match ledger_client
            .get_batch_by_slot_id_and_offset(&IntOrHash::Integer(rollup_height), 0, None)
            .await
        {
            Ok(response) => response,
            Err(err) => {
                if err.status() == Some(StatusCode::NOT_FOUND) {
                    continue;
                }
                anyhow::bail!(err);
            }
        };

        let tx_range = batch_response.data.clone().unwrap().tx_range.clone();
        let txs_count = tx_range.end.saturating_sub(tx_range.start);
        // TODO: Later we can assert `submitted_batch_info.batch_hash` with `batch_response.data.hash`.
        if txs_count > 0 {
            return Ok(rollup_height);
        }
    }

    anyhow::bail!(
        "Couldn't reach rollup height being published after {} slots",
        wait_for
    );
}
