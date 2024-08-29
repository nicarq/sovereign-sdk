mod bank_periodic_da_tests;
mod bank_tests;
mod helpers;
use std::sync::Arc;

use anyhow::Context;
use demo_stf::authentication::ModAuth;
use futures::StreamExt;
use sov_mock_da::storable::service::StorableMockDaService;
use sov_mock_da::MockDaSpec;
use sov_modules_api::capabilities::Authenticator;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{Batch, RawTx};
use sov_rollup_interface::node::da::{DaService, DaServiceWithRetries};
use sov_test_utils::{ApiClient, TestSpec};

const TOKEN_SALT: u64 = 0;
const TOKEN_NAME: &str = "test_token";

trait TxSender {
    async fn send_txs(
        &self,
        client: &ApiClient,
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
        client: &ApiClient,
        transactions: &[Transaction<TestSpec>],
    ) -> anyhow::Result<u64> {
        let authenticated_txs = transactions
            .iter()
            .map(|signed_tx| ModAuth::<TestSpec, MockDaSpec>::encode(borsh::to_vec(&signed_tx)?))
            .collect::<anyhow::Result<Vec<RawTx>>>()?;

        let batch = Batch::new(authenticated_txs);
        let batch_bytes = borsh::to_vec(&batch)?;

        let fee = self.da_service.estimate_fee(batch_bytes.len()).await?;

        let mut slot_subscription = client
            .ledger
            .subscribe_slots()
            .await
            .context("Failed to subscribe to slots!")?;
        self.da_service.send_transaction(&batch_bytes, fee).await?;

        let slot_number = slot_subscription
            .next()
            .await
            .transpose()?
            .map(|slot| slot.number)
            .unwrap_or_default();
        Ok(slot_number)
    }
}
struct SequencerTxSender;

impl TxSender for SequencerTxSender {
    async fn send_txs(
        &self,
        client: &ApiClient,
        transactions: &[Transaction<TestSpec>],
    ) -> anyhow::Result<u64> {
        let mut slot_subscription = client
            .ledger
            .subscribe_slots()
            .await
            .context("Failed to subscribe to slots!")?;

        client
            .sequencer
            .publish_batch_with_serialized_txs(transactions)
            .await?;

        let slot_number = slot_subscription
            .next()
            .await
            .transpose()?
            .map(|slot| slot.number)
            .unwrap_or_default();

        Ok(slot_number)
    }
}
