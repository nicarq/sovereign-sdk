//! Utilities for the sequencer RPC.

use borsh::BorshSerialize;
use jsonrpsee::core::client::{ClientT, Subscription, SubscriptionClientT};
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::ws_client::{WsClient, WsClientBuilder};
use serde::de::DeserializeOwned;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::Spec;
use sov_rollup_interface::common::HexHash;
use sov_rollup_interface::services::batch_builder::TxHash;
use tracing::info;

use crate::tx_status::TxStatus;

/// A simple client for the sequencer RPC.
pub struct SimpleClient {
    http_client: HttpClient,
    ws_client: WsClient,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SubmitTransaction {
    body: Vec<u8>,
}

impl SimpleClient {
    /// Creates a new client at the given endpoint
    pub async fn new(address: &str, port: u16) -> Result<Self, anyhow::Error> {
        let http_client = HttpClientBuilder::default()
            .build(format!("http://{address}:{port}"))
            .unwrap();
        let ws_client = WsClientBuilder::default()
            .build(&format!("ws://{address}:{port}"))
            .await?;
        Ok(Self {
            http_client,
            ws_client,
        })
    }

    /// Sends a transaction to the sequencer for immediate publication.
    pub async fn send_transaction<Tx: BorshSerialize>(&self, tx: Tx) -> Result<(), anyhow::Error> {
        let args = vec![tx.try_to_vec()?];

        let submit_response: serde_json::Value =
            self.http_client.request("sequencer_acceptTx", args).await?;
        info!(submit_response = ?submit_response, "Got response from `sequencer_acceptTx");

        let arg: &[u8] = &[];
        let publish_response: String = self
            .http_client
            .request("sequencer_publishBatch", arg)
            .await?;
        info!(
            ?publish_response,
            "Got a response from `sequencer_publishBatch`"
        );
        Ok(())
    }

    /// Sends multiple transactions to the sequencer for immediate publication.
    pub async fn send_transactions<S: Spec>(
        &self,
        txs: &[Transaction<S>],
    ) -> Result<(), anyhow::Error> {
        for tx in txs {
            let request = SubmitTransaction {
                body: tx.try_to_vec()?,
            };
            let response: serde_json::Value = self
                .http_client
                .request("sequencer_acceptTx", vec![request])
                .await?;
            info!(?response, "response from sequencer_acceptTx");
        }

        let arg: &[u8] = &[];
        let response: serde_json::Value = self
            .http_client
            .request("sequencer_publishBatch", arg)
            .await?;
        info!(?response, "Got a response from `sequencer_publishBatch`");

        Ok(())
    }

    /// Subscribes to transaction status updates for the given transaction hash.
    pub async fn subscribe_to_tx_status_updates<DaTxId: DeserializeOwned>(
        &self,
        tx_hash: TxHash,
    ) -> anyhow::Result<Subscription<TxStatus<DaTxId>>> {
        let sub = self
            .ws_client
            .subscribe(
                "sequencer_subscribeToTxStatusUpdates",
                &[HexHash::new(tx_hash)] as &[_],
                "sequencer_unsubscribeToTxStatusUpdates",
            )
            .await?;
        Ok(sub)
    }

    /// Get a reference to the underlying [`HttpClient`]
    pub fn http(&self) -> &HttpClient {
        &self.http_client
    }

    /// Get a reference to the underlying [`WsClient`]
    pub fn ws(&self) -> &WsClient {
        &self.ws_client
    }
}
