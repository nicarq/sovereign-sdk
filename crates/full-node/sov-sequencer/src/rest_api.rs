use std::sync::Arc;

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
use sov_modules_api::capabilities::TransactionAuthenticator;
use sov_modules_api::RawTx;
use sov_rest_utils::{
    errors, json_obj, preconfigured_router_layers, serve_generic_ws_subscription, to_json_object,
    ApiResult, ErrorObject, Path,
};
use sov_rollup_interface::da::{DaBlobHash, DaSpec};
use sov_rollup_interface::node::da::DaService;
use sov_rollup_interface::TxHash;
use tokio_stream::wrappers::BroadcastStream;

use crate::common::Sequencer;
use crate::{SequencerNotReadyDetails, SubmitBatchReceipt, TxStatus};

/// Provides REST APIs for any [`Sequencer`]. See [`SequencerApis::rest_api_server`].
#[derive(derivative::Derivative)]
#[derivative(Clone(bound = ""))]
pub struct SequencerApis<Seq: Sequencer>(Arc<Seq>);

impl<Seq: Sequencer> SequencerApis<Seq> {
    /// Creates a new Axum router for this sequencer.
    pub fn rest_api_server(seq: Arc<Seq>) -> axum::Router<()> {
        let state = Self(seq);
        let routes_that_require_synced_node = axum::Router::new()
            .route("/txs", axum::routing::post(Self::axum_accept_tx))
            .route("/batches", axum::routing::post(Self::axum_submit_batch))
            .with_state(state.clone())
            .layer(middleware::from_fn_with_state(
                state.clone(),
                Self::ready_middleware,
            ));
        let routes_always_available = axum::Router::new()
            .route("/ready", axum::routing::get(Self::axum_get_ready))
            .route("/txs/:tx_hash", axum::routing::get(Self::axum_get_tx))
            .route("/txs/:tx_hash/ws", axum::routing::get(Self::axum_get_tx_ws))
            .route("/events/ws", axum::routing::get(Self::subscribe_to_events))
            .with_state(state.clone());

        preconfigured_router_layers(
            axum::Router::new()
                .nest("/sequencer", routes_that_require_synced_node)
                .nest("/sequencer", routes_always_available),
        )
    }

    async fn ready_middleware(
        State(sequencer): State<Self>,
        request: Request,
        next: Next,
    ) -> Result<Response, Response> {
        match sequencer.0.is_ready() {
            Ok(()) => Ok(next.run(request).await),
            Err(details) => Err(error_not_fully_synced(details).into_response()),
        }
    }

    async fn send_initial_status_to_ws(
        &self,
        tx_hash: TxHash,
        socket: &mut WebSocket,
    ) -> anyhow::Result<()> {
        // Send a message with the initial status of the transaction,
        // without waiting for it to change for the first time.
        let initial_status = self.0.tx_status(&tx_hash).await?;
        let ws_msg = ws::Message::Text(serde_json::to_string(&TxInfo {
            id: tx_hash,
            status: initial_status,
        })?);
        socket.send(ws_msg).await?;

        Ok(())
    }

    async fn axum_get_tx_ws(
        sequencer: State<Self>,
        tx_hash: Path<TxHash>,
        ws: ws::WebSocketUpgrade,
    ) -> impl IntoResponse {
        let tx_status_manager = sequencer.0 .0.tx_status_manager().clone();

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

    async fn axum_get_ready(sequencer: State<Self>) -> ApiResult<()> {
        match sequencer.0 .0.is_ready() {
            Ok(()) => Ok(().into()),
            Err(details) => Err(error_not_fully_synced(details).into_response()),
        }
    }

    async fn axum_get_tx(
        sequencer: State<Self>,
        tx_hash: Path<TxHash>,
    ) -> ApiResult<TxInfo<<<Seq::Da as DaService>::Spec as DaSpec>::TransactionId>> {
        let tx_status = sequencer.0 .0.tx_status(&tx_hash.0).await;

        if let Ok(tx_status) = tx_status {
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
    ) -> ApiResult<TxInfo<DaBlobHash<<Seq::Da as DaService>::Spec>>> {
        let raw_tx = RawTx::new(tx.0.body.blob);
        let baked_tx =
            <Seq::Rt as TransactionAuthenticator<Seq::Spec>>::encode_with_standard_auth(raw_tx);

        let tx_with_hash = sequencer
            .0
             .0
            .accept_tx(baked_tx)
            .await
            .map_err(IntoResponse::into_response)?;

        Ok(TxInfo {
            id: tx_with_hash.tx_hash,
            status: TxStatus::Submitted,
        }
        .into())
    }

    async fn axum_submit_batch(
        sequencer: State<Self>,
        batch: Json<SubmitBatch>,
    ) -> ApiResult<SubmitBatchReceipt<<Seq::Da as DaService>::Spec>> {
        let batch = batch
            .0
            .transactions
            .into_iter()
            .map(|tx| {
                let raw_tx = RawTx::new(tx.blob);
                <Seq::Rt as TransactionAuthenticator<Seq::Spec>>::encode_with_standard_auth(raw_tx)
            })
            .collect::<Vec<_>>();

        match sequencer.0.0.submit_batch(batch).await {
            Ok(Some(info)) => Ok(info.into()),
            Ok(None) => Err(ErrorObject {
                status: StatusCode::BAD_REQUEST,
                title: "Can't produce a batch at this time, wait until the DA has progressed more slots or ensure that valid transactions are available".to_string(),
                details: json_obj!({}),
            }.into_response()),
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
            let stream = sequencer
                .0
                .subscribe_events()
                .await
                .map(|receiver| {
                    BroadcastStream::new(receiver)
                        .map_err(|err| anyhow::anyhow!("Error creating broadcast stream: {err}"))
                        .boxed()
                })
                .unwrap_or_else(|| futures::stream::empty().boxed());
            serve_generic_ws_subscription(socket, stream).await;
        })
    }
}

fn error_not_fully_synced(details: SequencerNotReadyDetails) -> ErrorObject {
    ErrorObject {
        status: StatusCode::SERVICE_UNAVAILABLE,
        title: "The node is not fully synced with the DA head and can't accept transactions at this time; try again later"
            .to_string(),
        details: to_json_object(details)
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
