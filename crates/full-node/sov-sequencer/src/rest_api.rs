use std::sync::OnceLock;

use anyhow::Context;
use axum::extract::ws::WebSocket;
use axum::extract::{ws, Request, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{middleware, Json};
use futures::{StreamExt, TryStreamExt};
use serde_with::base64::Base64;
use serde_with::serde_as;
use sov_rest_utils::{
    errors, json_obj, preconfigured_router_layers, serve_generic_ws_subscription, ApiResult,
    ErrorObject, Path,
};
use sov_rollup_interface::da::{DaBlobHash, DaSpec};
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::TxHash;
use tokio_stream::wrappers::BroadcastStream;
use tracing::error;
use utoipa_swagger_ui::{Config, SwaggerUi};

use crate::batch_builders::AcceptTxError;
use crate::{Sequencer, SequencerSpec, SubmittedBatchInfo, TxStatus};

const RAW_YAML_SPEC: &str = include_str!("../openapi-v3.yaml");

/// This function does a pretty expensive clone of the entire OpenAPI
/// specification object, so it might be slow.
pub(crate) fn openapi_spec() -> serde_json::Value {
    static OPENAPI_SPEC: OnceLock<serde_json::Value> = OnceLock::new();

    OPENAPI_SPEC
        .get_or_init(|| serde_yaml::from_str::<serde_json::Value>(RAW_YAML_SPEC).unwrap())
        .clone()
}

/// Returns parsed [`openapiv3::OpenAPI`] for Sequencer JSON API.
/// Performs clone of the whole spec, so might be slow.
pub fn open_api_v3_spec() -> openapiv3::OpenAPI {
    static OPENAPI_SPEC_V3: OnceLock<openapiv3::OpenAPI> = OnceLock::new();
    OPENAPI_SPEC_V3
        .get_or_init(|| serde_yaml::from_str(RAW_YAML_SPEC).unwrap())
        .clone()
}

// Web server and Axum-related methods.
impl<Ss: SequencerSpec> Sequencer<Ss> {
    /// Creates a new Axum router for this sequencer.
    pub fn rest_api_server(&self, path_prefix: &str) -> axum::Router<()> {
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
                .route("/txs/:tx_hash", axum::routing::get(Self::axum_get_tx))
                .route("/txs/:tx_hash/ws", axum::routing::get(Self::axum_get_tx_ws))
                .route("/txs", axum::routing::post(Self::axum_accept_tx))
                .route("/batches", axum::routing::post(Self::axum_submit_batch))
                .route("/events/ws", axum::routing::get(Self::subscribe_to_events))
                .with_state(self.clone()),
        )
        .layer(middleware::from_fn_with_state(
            self.clone(),
            Sequencer::<Ss>::ready_middleware,
        ))
        .fallback(errors::global_404)
    }

    async fn ready_middleware(
        State(sequencer): State<Self>,
        request: Request,
        next: Next,
    ) -> Result<Response, Response> {
        if sequencer.is_ready().await {
            Ok(next.run(request).await)
        } else {
            Err(ErrorObject {
                status: StatusCode::SERVICE_UNAVAILABLE,
                title: "The node is not fully synced with the DA head and can't accept transactions at this time; try again later"
                    .to_string(),
                details: Default::default(),
            }
            .into_response())
        }
    }

    async fn send_initial_status_to_ws(
        &self,
        tx_hash: TxHash,
        socket: &mut WebSocket,
    ) -> anyhow::Result<()> {
        // Send a message with the initial status of the transaction,
        // without waiting for it to change for the first time.
        let initial_status = self.tx_status(&tx_hash).await?.unwrap_or(TxStatus::Unknown);
        let ws_msg = ws::Message::Text(serde_json::to_string(&TxInfo {
            id: tx_hash,
            status: initial_status,
        })?);
        dbg!(&ws_msg);
        socket.send(ws_msg).await?;

        Ok(())
    }

    async fn axum_get_tx_ws(
        sequencer: State<Self>,
        tx_hash: Path<TxHash>,
        ws: ws::WebSocketUpgrade,
    ) -> impl IntoResponse {
        let tx_status_manager = sequencer.tx_status_manager().clone();

        ws.on_upgrade(move |mut socket| async move {
            let (_dropper, receiver) = tx_status_manager.subscribe(tx_hash.0);

            // After "terminal" tx status updates (i.e. after which
            // we'll no longer send any new notifications), we close the
            // connection.
            let subscription = futures::stream::unfold(
                // We use the state to keep track of whether or not the last notification
                // was terminal.
                //
                // By wrapping the `receiver` in a `BroadcastStream`, we
                // ensure it'll be dropped before `_dropper`.
                (false, BroadcastStream::new(receiver)),
                |(terminated, mut stream)| async move {
                    if terminated {
                        None
                    } else {
                        let next = stream.next().await?;
                        let is_terminal: bool = next
                            .as_ref()
                            .map(|status| status.is_terminal())
                            // Errors result in WebSocket connection termination.
                            .unwrap_or(true);
                        Some((next, (is_terminal, stream)))
                    }
                },
            )
            // Finally, convert the data into the type that we want to
            // serialize over the WS connection.
            .map(|data| {
                data.context("Failed to subscribe to tx status updates")
                    .map(|status| TxInfo {
                        id: tx_hash.0,
                        status,
                    })
            })
            .boxed();

            sequencer
                .send_initial_status_to_ws(tx_hash.0, &mut socket)
                .await
                .ok();

            serve_generic_ws_subscription(socket, subscription).await;
        })
    }

    async fn axum_get_tx(
        sequencer: State<Self>,
        tx_hash: Path<TxHash>,
    ) -> ApiResult<TxInfo<<<Ss::Da as DaService>::Spec as DaSpec>::TransactionId>> {
        let tx_status = sequencer.tx_status_manager().get_cached(&tx_hash.0);

        if let Some(tx_status) = tx_status {
            Ok(TxInfo {
                id: tx_hash.0,
                status: tx_status,
            }
            .into())
        } else {
            Err(errors::not_found_404("Transaction", tx_hash.0))
        }
    }

    async fn axum_accept_tx(
        sequencer: State<Self>,
        tx: Json<AcceptTx>,
    ) -> ApiResult<TxInfo<DaBlobHash<<Ss::Da as DaService>::Spec>>> {
        let tx = tx.0.body.blob;

        let tx_with_hash = match sequencer.accept_tx(tx).await {
            Ok(tx_hash) => tx_hash,
            Err(AcceptTxError {
                http_status,
                title,
                details,
            }) => {
                return Err(ErrorObject {
                    status: http_status.try_into().unwrap_or_else(|_| {
                        error!(
                            http_status,
                            "Sequencer generated an invalid HTTP status code"
                        );
                        StatusCode::INTERNAL_SERVER_ERROR
                    }),
                    title,
                    details: json_obj!({
                        "message": details
                    }),
                }
                .into_response());
            }
        };

        Ok(TxInfo {
            id: tx_with_hash.tx_hash,
            status: TxStatus::Submitted,
        }
        .into())
    }

    async fn axum_submit_batch(
        sequencer: State<Self>,
        batch: Json<SubmitBatch>,
    ) -> ApiResult<SubmittedBatchInfo> {
        let batch = batch
            .0
            .transactions
            .into_iter()
            .map(|tx| tx.blob)
            .collect::<Vec<_>>();

        match sequencer.submit_batch(batch).await {
            Ok(info) => Ok(info.into()),
            Err(err) => Err(ErrorObject {
                status: StatusCode::CONFLICT,
                title: "Failed to submit batch".to_string(),
                details: json_obj!({
                    "message": err.to_string(),
                }),
            }
            .into_response()),
        }
    }

    async fn subscribe_to_events(
        State(sequencer): State<Self>,
        ws: WebSocketUpgrade,
    ) -> impl IntoResponse {
        ws.on_upgrade(|socket| async move {
            let receiver = sequencer.subscribe_events().await;
            let stream = BroadcastStream::new(receiver)
                .map_err(|err| anyhow::anyhow!("Error creating broadcast stream: {err}"))
                .boxed();
            serve_generic_ws_subscription(socket, stream).await;
        })
    }
}

#[serde_as]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Base64Blob {
    #[serde_as(as = "Base64")]
    blob: Vec<u8>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct AcceptTx {
    pub body: Base64Blob,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct SubmitBatch {
    pub transactions: Vec<Base64Blob>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct TxInfo<DaTransactionId> {
    id: TxHash,
    #[serde(flatten)]
    status: TxStatus<DaTransactionId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_spec_is_valid() {
        let _spec = openapi_spec();
    }
}
