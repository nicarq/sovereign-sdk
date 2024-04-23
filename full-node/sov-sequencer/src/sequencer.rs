use std::sync::Arc;

use borsh::BorshSerialize;
use jsonrpsee::core::StringError;
use jsonrpsee::types::ErrorObjectOwned;
use jsonrpsee::{PendingSubscriptionSink, RpcModule, SubscriptionMessage};
use serde::Serialize;
use sov_modules_api::batch::Batch;
use sov_modules_api::runtime::capabilities::RawTx;
use sov_modules_api::utils::to_jsonrpsee_error_object;
use sov_rollup_interface::da::BlockHeaderTrait;
use sov_rollup_interface::services::batch_builder::{BatchBuilder, TxHash};
use sov_rollup_interface::services::da::DaService;
use tokio::sync::Mutex;
use tracing::info;

use super::tx_status::{TxStatus, TxStatusNotifier};
use super::{AcceptTxResponse, HexHash, SubmittedBatchInfo};

const SEQUENCER_RPC_ERROR: &str = "SEQUENCER_RPC_ERROR";

/// Single data structure that manages mempool and batch producing.
pub struct Sequencer<B: BatchBuilder, Da: DaService>(Arc<Inner<B, Da>>);

impl<B, Da> Clone for Sequencer<B, Da>
where
    B: BatchBuilder,
    Da: DaService,
{
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

struct Inner<B: BatchBuilder, Da: DaService> {
    batch_builder: Mutex<B>,
    da_service: Da,
    tx_status_notifier: Arc<TxStatusNotifier<Da>>,
}

impl<B, Da> Sequencer<B, Da>
where
    B: BatchBuilder + Send + Sync + 'static,
    Da: DaService,
    Da::TransactionId: Clone + Send + Sync + serde::Serialize,
{
    /// Creates new Sequencer from BatchBuilder and DaService
    pub fn new(batch_builder: B, da_service: Da) -> Self {
        Self(Arc::new(Inner {
            batch_builder: Mutex::new(batch_builder),
            da_service,
            tx_status_notifier: Arc::new(TxStatusNotifier::new()),
        }))
    }

    async fn submit_batch(&self, txs: Vec<Vec<u8>>) -> anyhow::Result<SubmittedBatchInfo> {
        // Acquire the lock before any DA operation, to avoid out-of-order
        // batches and other potential issues.
        let mut batch_builder = self.0.batch_builder.lock().await;

        let mut accept_tx_results = vec![];
        for tx in txs {
            let result = batch_builder
                .accept_tx(tx.clone())
                .await
                .map(|tx_hash| {
                    self.0
                        .tx_status_notifier
                        .notify(tx_hash, TxStatus::Submitted);
                    AcceptTxResponse {
                        tx,
                        tx_hash: HexHash(tx_hash),
                    }
                })
                .map_err(|e| to_jsonrpsee_error_object(e, SEQUENCER_RPC_ERROR));
            accept_tx_results.push(result);
        }

        tracing::info!("Submit batch request has been received!");

        let da_height = self
            .0
            .da_service
            .get_head_block_header()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch current head: {}", e))?
            .height();

        let blob_txs = batch_builder.get_next_blob(da_height).await?;
        let num_txs = blob_txs.len();
        let (txs, tx_hashes) = blob_txs
            .into_iter()
            .map(|tx| (RawTx { data: tx.raw_tx }, tx.hash))
            .unzip::<_, _, Vec<_>, Vec<_>>();

        let batch = Batch { txs };
        let serialized_batch = batch.try_to_vec()?;

        let da_tx_id = match self.0.da_service.send_transaction(&serialized_batch).await {
            Ok(id) => id,
            Err(e) => anyhow::bail!("failed to submit batch: {}", e),
        };

        for tx_hash in tx_hashes {
            self.0.tx_status_notifier.notify(
                tx_hash,
                TxStatus::Published {
                    da_transaction_id: da_tx_id.clone(),
                },
            );
        }

        Ok(SubmittedBatchInfo { da_height, num_txs })
    }

    async fn accept_tx(&self, tx: Vec<u8>) -> anyhow::Result<TxHash> {
        let mut batch_builder = self.0.batch_builder.lock().await;

        info!(tx = hex::encode(&tx), "Accepting transaction");
        let tx_hash = batch_builder.accept_tx(tx).await?;
        self.0
            .tx_status_notifier
            .notify(tx_hash, TxStatus::Submitted);

        Ok(tx_hash)
    }

    async fn tx_status(
        &self,
        tx_hash: &TxHash,
    ) -> anyhow::Result<Option<TxStatus<Da::TransactionId>>> {
        let is_in_mempool = self.0.batch_builder.lock().await.contains(tx_hash).await?;

        if is_in_mempool {
            Ok(Some(TxStatus::Submitted))
        } else {
            Ok(self.0.tx_status_notifier.get_cached(tx_hash))
        }
    }
}

mod jsonrpc {
    use super::*;

    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct SubmitTransaction {
        pub body: Vec<u8>,
    }

    impl<B, Da> Sequencer<B, Da>
    where
        B: BatchBuilder + Send + Sync + 'static,
        Da: DaService,
        Da::TransactionId: Clone + Send + Sync + serde::Serialize,
    {
        /// Returns the [`jsonrpsee::RpcModule`] for the sequencer-related RPC
        /// methods.
        pub fn rpc(&self) -> RpcModule<Self> {
            let mut rpc = RpcModule::new(self.clone());
            Self::register_txs_rpc_methods(&mut rpc)
                .expect("Failed to register sequencer RPC methods");
            rpc
        }

        fn register_txs_rpc_methods(
            rpc: &mut RpcModule<Self>,
        ) -> Result<(), jsonrpsee::core::Error> {
            rpc.register_async_method(
                "sequencer_publishBatch",
                |params, batch_builder| async move {
                    let mut params_iter = params.sequence();
                    let mut txs = vec![];
                    while let Some(tx) = params_iter.optional_next()? {
                        txs.push(tx);
                    }
                    let submitted_batch_info = batch_builder
                        .submit_batch(txs)
                        .await
                        .map_err(|e| to_jsonrpsee_error_object(e, SEQUENCER_RPC_ERROR))?;

                    Ok::<SubmittedBatchInfo, ErrorObjectOwned>(submitted_batch_info)
                },
            )?;
            rpc.register_async_method("sequencer_acceptTx", |params, sequencer| async move {
                let tx = params.one::<SubmitTransaction>()?.body;

                sequencer
                    .accept_tx(tx.clone())
                    .await
                    .map(|tx_hash| AcceptTxResponse {
                        tx,
                        tx_hash: HexHash(tx_hash),
                    })
                    .map_err(|e| to_jsonrpsee_error_object(e, SEQUENCER_RPC_ERROR))
            })?;

            rpc.register_async_method("sequencer_txStatus", |params, sequencer| async move {
                let tx_hash: HexHash = params.one()?;

                let status = sequencer
                    .tx_status(&tx_hash.0)
                    .await
                    .map_err(|e| to_jsonrpsee_error_object(e, SEQUENCER_RPC_ERROR))?;
                Ok::<_, ErrorObjectOwned>(status)
            })?;
            rpc.register_subscription(
                "sequencer_subscribeToTxStatusUpdates",
                "sequencer_newTxStatus",
                "sequencer_unsubscribeToTxStatusUpdates",
                |params, pending, sequencer| async move {
                    Self::handle_tx_status_update_subscription(&sequencer, params, pending).await
                },
            )?;

            Ok(())
        }

        async fn handle_tx_status_update_subscription(
            &self,
            params: jsonrpsee::types::Params<'_>,
            sink: PendingSubscriptionSink,
        ) -> Result<(), StringError> {
            let tx_hash: HexHash = params.one()?;
            let mut receiver = self.0.tx_status_notifier.clone().subscribe(tx_hash.0);

            let subscription = sink.accept().await?;

            let initial_status = self
                .tx_status(&tx_hash.0)
                .await?
                .unwrap_or(TxStatus::Unknown);
            subscription
                .send(SubscriptionMessage::from_json(&initial_status)?)
                .await?;

            while let Ok(new_status) = receiver.recv.recv().await {
                let notification = SubscriptionMessage::from_json(&new_status)?;
                subscription.send(notification).await?;
            }

            Ok(())
        }
    }
}

mod axum_router {
    use std::sync::OnceLock;

    use axum::extract::{ws, State};
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::Json;
    use serde_with::base64::Base64;
    use serde_with::serde_as;
    use sov_jsonapi_utils::types::{ErrorObject, JsonObject, ResponseObject};
    use sov_jsonapi_utils::utils::{not_found_404, preconfigured_router_layers};
    use sov_jsonapi_utils::{json_obj, PathWithErrorHandling};
    use tracing::debug;
    use utoipa_swagger_ui::{Config, SwaggerUi};

    use super::*;

    /// This function does a pretty expensive clone of the entire OpenAPI
    /// specification object, so it might be slow.
    pub(crate) fn openapi_spec() -> serde_json::Value {
        static OPENAPI_SPEC: OnceLock<serde_json::Value> = OnceLock::new();

        OPENAPI_SPEC
            .get_or_init(|| {
                let openapi_spec_raw_yaml_contents = include_str!("../openapi-v3.yaml");
                serde_yaml::from_str::<serde_json::Value>(openapi_spec_raw_yaml_contents).unwrap()
            })
            .clone()
    }

    #[serde_as]
    #[derive(serde::Serialize, serde::Deserialize)]
    #[serde(transparent)]
    pub struct Base64Blob {
        #[serde_as(as = "Base64")]
        blob: Vec<u8>,
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct SubmitBatch {
        pub transactions: Vec<Base64Blob>,
    }

    fn tx_attributes<DaTxId: Serialize>(hash: HexHash, status: TxStatus<DaTxId>) -> JsonObject {
        json_obj!({
            "id": hash.to_string(),
            "status": status,
        })
    }

    // Web server and Axum-related methods.
    impl<B, Da> Sequencer<B, Da>
    where
        B: BatchBuilder + Send + Sync + 'static,
        Da: DaService,
        Da::TransactionId: Clone + Send + Sync + serde::Serialize,
    {
        /// Creates an Axum router for the sequencer.
        pub fn axum_router(&self, path_prefix: &str) -> axum::Router<Self> {
            preconfigured_router_layers(
                axum::Router::new()
                    // See:
                    // - https://github.com/juhaku/utoipa/issues/599
                    // - https://github.com/juhaku/utoipa/issues/734
                    .merge(
                        SwaggerUi::new("/swagger-ui")
                            .external_url_unchecked("/openapi-v3.yaml", openapi_spec())
                            .config(Config::from(format!("{}/openapi-v3.yaml", path_prefix))),
                    )
                    .route("/txs", axum::routing::post(Self::axum_accept_tx))
                    .route("/txs/:tx_hash", axum::routing::get(Self::axum_get_tx))
                    .route("/txs/:tx_hash/ws", axum::routing::get(Self::axum_get_tx_ws))
                    .route("/batches", axum::routing::post(Self::axum_submit_batch)),
            )
        }

        async fn axum_get_tx_ws(
            sequencer: State<Self>,
            tx_hash: PathWithErrorHandling<HexHash>,
            ws: ws::WebSocketUpgrade,
        ) -> impl IntoResponse {
            let notifier = sequencer.0 .0.tx_status_notifier.clone();

            ws.on_upgrade(move |mut socket| async move {
                let mut tx_status_recv = notifier.subscribe(tx_hash.0 .0);

                while let Ok(tx_status) = tx_status_recv.recv.recv().await {
                    let resource_obj = tx_attributes(tx_hash.0, tx_status);
                    let ws_msg = ws::Message::Text(serde_json::to_string(&resource_obj).unwrap());

                    if let Err(error) = socket.send(ws_msg).await {
                        debug!(?error, "WebSocket connection closed (or errored)");
                        break;
                    }
                }
            })
        }

        async fn axum_get_tx(
            sequencer: State<Self>,
            tx_hash: PathWithErrorHandling<HexHash>,
        ) -> impl IntoResponse {
            let tx_status = sequencer.0 .0.tx_status_notifier.get_cached(&tx_hash.0 .0);

            if let Some(tx_status) = tx_status {
                let resource_obj = tx_attributes(tx_hash.0, tx_status);

                (
                    StatusCode::OK,
                    Json(ResponseObject {
                        data: Some(resource_obj.into()),
                        ..Default::default()
                    }),
                )
            } else {
                not_found_404("Transaction", tx_hash.0)
            }
        }

        async fn axum_accept_tx(sequencer: State<Self>, tx: Json<Base64Blob>) -> impl IntoResponse {
            let tx = tx.0.blob;

            let tx_hash = match sequencer.accept_tx(tx.clone()).await {
                Ok(tx_hash) => tx_hash,
                Err(err) => {
                    let response_obj = ResponseObject {
                        errors: vec![ErrorObject {
                            status: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                            title: "Failed to submit transaction".to_string(),
                            details: json_obj!({
                                "message": err.to_string(),
                            }),
                        }],
                        ..Default::default()
                    };

                    return Json(response_obj);
                }
            };

            let resource_obj =
                tx_attributes(HexHash(tx_hash), TxStatus::<Da::TransactionId>::Submitted);
            let response_obj = ResponseObject {
                data: Some(resource_obj.into()),
                ..Default::default()
            };
            Json(response_obj)
        }

        async fn axum_submit_batch(
            sequencer: State<Self>,
            batch: Json<SubmitBatch>,
        ) -> impl IntoResponse {
            let batch = batch.0.transactions.into_iter().map(|tx| tx.blob).collect();

            let submitted_batch_info = match sequencer.submit_batch(batch).await {
                Ok(info) => info,
                Err(err) => {
                    let response_obj = ResponseObject {
                        errors: vec![ErrorObject {
                            status: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                            title: "Failed to submit batch".to_string(),
                            details: json_obj!({
                                "message": err.to_string(),
                            }),
                        }],
                        ..Default::default()
                    };

                    return Json(response_obj);
                }
            };

            let response_obj = ResponseObject {
                data: Some(
                    json_obj!({
                        "daHeight": submitted_batch_info.da_height,
                        "numTxs": submitted_batch_info.num_txs,
                    })
                    .into(),
                ),
                ..Default::default()
            };

            Json(response_obj)
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use sov_mock_da::{MockAddress, MockDaService};
    use sov_rollup_interface::da::BlobReaderTrait;
    use sov_rollup_interface::services::batch_builder::TxWithHash;

    use self::axum_router::openapi_spec;
    use super::*;

    fn sequencer_rpc(
        batch_builder: MockBatchBuilder,
        da_service: MockDaService,
    ) -> RpcModule<Sequencer<MockBatchBuilder, MockDaService>> {
        Sequencer::new(batch_builder, da_service).rpc()
    }

    /// BatchBuilder used in tests.
    pub struct MockBatchBuilder {
        /// Mempool with transactions.
        pub mempool: Vec<Vec<u8>>,
    }

    // It only takes the first byte of the tx, when submits it.
    // This allows to show effect of batch builder
    #[async_trait]
    impl BatchBuilder for MockBatchBuilder {
        async fn accept_tx(&mut self, tx: Vec<u8>) -> anyhow::Result<TxHash> {
            self.mempool.push(tx);
            Ok([0; 32])
        }

        async fn contains(&self, _tx_hash: &TxHash) -> anyhow::Result<bool> {
            unimplemented!("MockBatchBuilder::contains is not implemented")
        }

        async fn get_next_blob(&mut self, _height: u64) -> anyhow::Result<Vec<TxWithHash>> {
            if self.mempool.is_empty() {
                anyhow::bail!("Mock mempool is empty");
            }
            let txs = std::mem::take(&mut self.mempool)
                .into_iter()
                .filter_map(|tx| {
                    let first_byte = *tx.first()?;
                    Some(TxWithHash {
                        raw_tx: vec![first_byte],
                        hash: [0; 32],
                    })
                })
                .collect();
            Ok(txs)
        }
    }

    #[test]
    fn openapi_spec_is_valid() {
        let _spec = openapi_spec();
    }

    #[tokio::test]
    async fn test_submit_on_empty_mempool() {
        let batch_builder = MockBatchBuilder { mempool: vec![] };
        let da_service = MockDaService::new(MockAddress::default());
        let rpc = sequencer_rpc(batch_builder, da_service);

        let arg: &[u8] = &[];
        let result: Result<String, jsonrpsee::core::Error> =
            rpc.call("sequencer_publishBatch", arg).await;

        assert!(result.is_err());
        let error = result.err().unwrap();
        assert_eq!(
            "ErrorObject { code: ServerError(-32001), message: \"SEQUENCER_RPC_ERROR\", data: Some(RawValue(\"Mock mempool is empty\")) }",
            error.to_string()
        );
    }

    #[tokio::test]
    async fn test_submit_happy_path() {
        let tx1 = vec![1, 2, 3];
        let tx2 = vec![3, 4, 5];
        // First bytes of each tx, flattened
        let blob: Vec<Vec<u8>> = vec![vec![tx1[0]], vec![tx2[0]]];
        let expected: Vec<u8> = borsh::to_vec(&blob).unwrap();

        let batch_builder = MockBatchBuilder {
            mempool: vec![tx1, tx2],
        };
        let da_service = MockDaService::new(MockAddress::default());
        let rpc = sequencer_rpc(batch_builder, da_service.clone());

        let arg: &[u8] = &[];
        let _: serde_json::Value = rpc.call("sequencer_publishBatch", arg).await.unwrap();

        let mut submitted_block = da_service.get_block_at(1).await.unwrap();
        let block_data = submitted_block.batch_blobs[0].full_data();

        assert_eq!(expected, block_data);
    }

    #[tokio::test]
    async fn test_accept_tx() {
        let batch_builder = MockBatchBuilder { mempool: vec![] };
        let da_service = MockDaService::new(MockAddress::default());

        let rpc = sequencer_rpc(batch_builder, da_service.clone());

        let tx: Vec<u8> = vec![1, 2, 3, 4, 5];
        // First bytes of each tx, flattened
        let blob: Vec<Vec<u8>> = vec![vec![tx[0]]];
        let expected_block_data: Vec<u8> = borsh::to_vec(&blob).unwrap();

        let request = super::jsonrpc::SubmitTransaction { body: tx };
        rpc.call::<_, AcceptTxResponse>("sequencer_acceptTx", [request])
            .await
            .unwrap();

        let arg: &[u8] = &[];
        let _: serde_json::Value = rpc.call("sequencer_publishBatch", arg).await.unwrap();

        let mut submitted_block = da_service.get_block_at(1).await.unwrap();
        let block_data = submitted_block.batch_blobs[0].full_data();

        assert_eq!(expected_block_data, block_data);
    }
}
