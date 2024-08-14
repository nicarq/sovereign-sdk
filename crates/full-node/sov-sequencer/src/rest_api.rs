use std::sync::OnceLock;

use anyhow::Context;
use axum::extract::ws::WebSocket;
use axum::extract::{ws, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use futures::StreamExt;
use serde_with::base64::Base64;
use serde_with::serde_as;
use sov_modules_api::capabilities::Authenticator;
use sov_rest_utils::{
    errors, json_obj, preconfigured_router_layers, serve_generic_ws_subscription, ApiResult,
    ErrorObject, Path,
};
use sov_rollup_interface::da::DaBlobHash;
use sov_rollup_interface::services::batch_builder::AcceptTxError;
use sov_rollup_interface::services::da::DaService;
use sov_rollup_interface::TxHash;
use tokio_stream::wrappers::BroadcastStream;
use tracing::error;
use utoipa_swagger_ui::{Config, SwaggerUi};

use crate::{Sequencer, SequencerSpec, SubmittedBatchInfo, TxStatus};

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

/// Creates a new Axum router for the sequencer.
pub fn sequencer_rest_api_server<Ss: SequencerSpec>(
    path_prefix: &str,
) -> axum::Router<Sequencer<Ss>> {
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
            .route("/txs", axum::routing::post(Sequencer::<Ss>::axum_accept_tx))
            .route(
                "/txs/:tx_hash",
                axum::routing::get(Sequencer::<Ss>::axum_get_tx),
            )
            .route(
                "/txs/:tx_hash/ws",
                axum::routing::get(Sequencer::<Ss>::axum_get_tx_ws),
            )
            .route(
                "/batches",
                axum::routing::post(Sequencer::<Ss>::axum_submit_batch),
            ),
    )
}

// Web server and Axum-related methods.
impl<Ss: SequencerSpec> Sequencer<Ss> {
    async fn send_initial_status_to_ws(
        &self,
        tx_hash: TxHash,
        socket: &mut WebSocket,
    ) -> anyhow::Result<()> {
        // Send a messge with the initial status of the transaction,
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
        let notifier = sequencer.notifier().clone();

        ws.on_upgrade(move |mut socket| async move {
            let (_dropper, receiver) = notifier.subscribe(tx_hash.0);

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
    ) -> ApiResult<TxInfo<DaBlobHash<<Ss::Da as DaService>::Spec>>> {
        let tx_status = sequencer.notifier().get_cached(&tx_hash.0);

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
        let authed_tx = Ss::Auth::encode(tx)
            .map_err(|e| errors::bad_request_400("Failed to encode transaction", e))?;

        let tx_hash = match sequencer.accept_tx(authed_tx.data).await {
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
            id: tx_hash,
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
            .map(|tx| Ok(Ss::Auth::encode(tx.blob)?.data))
            .collect::<anyhow::Result<Vec<_>>>()
            .map_err(|e| errors::bad_request_400("Failed to encode transaction(s)", e))?;

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
struct TxInfo<BlobHash> {
    id: TxHash,
    #[serde(flatten)]
    status: TxStatus<BlobHash>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_spec_is_valid() {
        let _spec = openapi_spec();
    }
}
