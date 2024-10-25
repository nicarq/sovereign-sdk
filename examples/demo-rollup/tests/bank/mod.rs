mod helpers;
mod op_rollup;
mod zk_rollup;
use std::sync::Arc;

use anyhow::Context;
use demo_stf::runtime::Runtime;
use futures::StreamExt;
use reqwest::StatusCode;
use sov_api_spec::types::{IntOrHash, Slot};
use sov_cli::NodeClient;
use sov_mock_da::storable::service::StorableMockDaService;
use sov_modules_api::capabilities::TransactionAuthenticator;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{Batch, RawTx};
use sov_rollup_interface::node::da::{DaService, DaServiceWithRetries};
use sov_test_utils::TestSpec;

const TOKEN_NAME: &str = "test_token";

trait TxSender {
    /// Returns rollup_height of the batch has been produced.
    async fn send_txs(
        &self,
        client: &NodeClient,
        transactions: &[Transaction<TestSpec>],
    ) -> anyhow::Result<u64>;
}

struct DaLayerTxSender {
    da_service: Arc<DaServiceWithRetries<StorableMockDaService>>,
}

impl DaLayerTxSender {
    fn new(da_service: Arc<DaServiceWithRetries<StorableMockDaService>>) -> Self {
        Self { da_service }
    }
}

impl TxSender for DaLayerTxSender {
    async fn send_txs(
        &self,
        client: &NodeClient,
        transactions: &[Transaction<TestSpec>],
    ) -> anyhow::Result<u64> {
        let authenticated_txs = transactions
            .iter()
            .map(|signed_tx| {
                Runtime::<TestSpec>::encode_with_standard_auth(RawTx::new(
                    borsh::to_vec(&signed_tx).unwrap(),
                ))
            })
            .collect::<Vec<_>>();

        let batch = Batch::new(authenticated_txs);
        let batch_bytes = borsh::to_vec(&batch)?;

        let fee = self.da_service.estimate_fee(batch_bytes.len()).await?;

        let slot_subscription = client
            .client
            .subscribe_slots()
            .await
            .context("Failed to subscribe to slots!")?;
        self.da_service.send_transaction(&batch_bytes, fee).await?;

        wait_for_batch_to_be_processed(slot_subscription, &client.client).await
    }
}

struct SequencerTxSender;

impl TxSender for SequencerTxSender {
    async fn send_txs(
        &self,
        client: &NodeClient,
        transactions: &[Transaction<TestSpec>],
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
    mut slot_subscription: futures::stream::BoxStream<'_, anyhow::Result<Slot>>,
    ledger_client: &sov_api_spec::Client,
) -> anyhow::Result<u64> {
    let wait_for = 1_000;
    for _ in 0..wait_for {
        let slot_number = slot_subscription
            .next()
            .await
            .transpose()?
            .map(|slot| slot.number)
            .unwrap_or_default();

        let batch_response = match ledger_client
            .get_batch_by_slot_id_and_offset(&IntOrHash::Integer(slot_number), 0, None)
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
            return Ok(slot_number);
        }
    }

    anyhow::bail!(
        "Couldn't reach slot number being published after {} slots",
        wait_for
    );
}
