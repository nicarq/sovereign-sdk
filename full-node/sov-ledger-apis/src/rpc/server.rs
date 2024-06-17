//! A JSON-RPC server implementation for any [`LedgerStateProvider`].

use futures::future::Either;
use jsonrpsee::types::ErrorObjectOwned;
use jsonrpsee::{PendingSubscriptionSink, RpcModule, SubscriptionMessage};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sov_modules_api::utils::to_jsonrpsee_error_object;
use sov_modules_api::LedgerStateProviderExt;
use sov_rollup_interface::common::HexHash;
use sov_rollup_interface::rpc::{
    AggregatedProofResponse, BatchIdentifier, EventIdentifier, LedgerStateProvider,
    ProofInfoResponse, QueryMode, SlotIdentifier, TxIdentifier,
};
use sov_rollup_interface::stf::{StoredEvent, TxReceiptContents};
use tokio::sync::broadcast::Receiver;

const LEDGER_RPC_ERROR: &str = "LEDGER_RPC_ERROR";

/// Type alias for `Option<(u64, u64)>` representing an optional start and end transaction range
pub type TxnRangeParam = Option<(u64, u64)>;

/// Creates a new [`RpcModule`] that exposes all JSON-RPC methods
/// necessary to interface with the [`LedgerStateProvider`].
///
/// # Example
/// ```
/// use std::sync::{Arc, RwLock};
/// use sov_ledger_apis::rpc::server::rpc_module;
/// use tempfile::tempdir;
/// use sov_db::ledger_db::LedgerDb;
/// use sov_db::schema::{CacheContainer, CacheDb};
/// use sov_test_utils::TestSpec;
/// use demo_stf::runtime::Runtime;
/// use sov_mock_da::MockDaSpec;
/// use sov_modules_api::{RuntimeEventResponse,RuntimeEventProcessor};
///
/// /// Creates a new [`LedgerDb`] and starts serving JSON-RPC requests.
/// async fn rpc_server() -> jsonrpsee::server::ServerHandle {
///     let dir = tempdir().unwrap();
///     let schema_db = LedgerDb::get_rockbound_options()
///             .default_setup_db_in_path(dir.path())
///             .unwrap();
///     let cache_container = CacheContainer::new(schema_db, Arc::new(RwLock::new(Default::default())).into());
///     let cache_db = CacheDb::new(0, Arc::new(RwLock::new(cache_container)).into());
///     let ledger_db = LedgerDb::with_cache_db(cache_db).unwrap();
///     let rpc_module = rpc_module::<LedgerDb, u32, (), RuntimeEventResponse<<Runtime<TestSpec, MockDaSpec> as RuntimeEventProcessor>::RuntimeEvent>>(ledger_db).unwrap();
///
///     let server = jsonrpsee::server::ServerBuilder::default()
///         .build("127.0.0.1:0")
///         .await
///         .unwrap();
///     server.start(rpc_module)
/// }
/// ```
pub fn rpc_module<T, B, Tx, E>(ledger: T) -> anyhow::Result<RpcModule<T>>
where
    T: LedgerStateProvider + LedgerStateProviderExt + Send + Sync + 'static,
    B: serde::Serialize + DeserializeOwned + Clone + Send + Sync + 'static,
    Tx: TxReceiptContents,
    E: TryFrom<StoredEvent, Error = anyhow::Error>
        + serde::Serialize
        + DeserializeOwned
        + Clone
        + Send
        + Sync
        + 'static,
{
    let mut rpc = RpcModule::new(ledger);

    rpc.register_async_method("ledger_getHead", |params, ledger| async move {
        let mut params = params.sequence();
        let query_mode = params.optional_next()?.unwrap_or(QueryMode::Compact);
        ledger
            .get_head::<B, Tx>(query_mode)
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
    })?;

    // Primary getters.
    rpc.register_async_method("ledger_getSlots", |params, ledger| async move {
        let args: QueryArgs<Vec<SlotIdentifier>> = extract_query_args(params)?;
        ledger
            .get_slots::<B, Tx>(&args.0, args.1)
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
    })?;
    rpc.register_async_method("ledger_getBatches", |params, ledger| async move {
        let args: QueryArgs<Vec<BatchIdentifier>> = extract_query_args(params)?;
        ledger
            .get_batches::<B, Tx>(&args.0, args.1)
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
    })?;
    rpc.register_async_method("ledger_getTransactions", |params, ledger| async move {
        let args: QueryArgs<Vec<TxIdentifier>> = extract_query_args(params)?;
        ledger
            .get_transactions::<Tx>(&args.0, args.1)
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
    })?;
    rpc.register_async_method("ledger_getEvents", |params, db| async move {
        let ids: Vec<EventIdentifier> = params.parse().or_else(|_| params.one())?;
        db.get_events::<E>(&ids)
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
    })?;

    // By-hash getters.
    rpc.register_async_method("ledger_getSlotByHash", |params, ledger| async move {
        let args: QueryArgs<HexHash> = extract_query_args(params)?;
        ledger
            .get_slot_by_hash::<B, Tx>(&args.0 .0, args.1)
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
    })?;
    rpc.register_async_method("ledger_getBatchByHash", |params, ledger| async move {
        let args: QueryArgs<HexHash> = extract_query_args(params)?;
        ledger
            .get_batch_by_hash::<B, Tx>(&args.0 .0, args.1)
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
    })?;
    rpc.register_async_method("ledger_getTransactionByHash", |params, ledger| async move {
        let args: QueryArgs<HexHash> = extract_query_args(params)?;
        ledger
            .get_tx_by_hash::<Tx>(&args.0 .0, args.1)
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
    })?;

    rpc.register_async_method(
        "ledger_getTransactionNumbersByHash",
        |params, ledger| async move {
            let args: QueryArgs<HexHash> = extract_query_args(params)?;
            ledger
                .get_tx_numbers_by_hash(&args.0 .0)
                .await
                .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
        },
    )?;

    // By-number getters.
    rpc.register_async_method("ledger_getSlotByNumber", |params, ledger| async move {
        let args: QueryArgs<u64> = extract_query_args(params)?;
        ledger
            .get_slot_by_number::<B, Tx>(args.0, args.1)
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
    })?;
    rpc.register_async_method("ledger_getBatchByNumber", |params, ledger| async move {
        let args: QueryArgs<u64> = extract_query_args(params)?;
        ledger
            .get_batch_by_number::<B, Tx>(args.0, args.1)
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
    })?;
    rpc.register_async_method(
        "ledger_getTransactionByNumber",
        |params, ledger| async move {
            let args: QueryArgs<u64> = extract_query_args(params)?;
            ledger
                .get_tx_by_number::<Tx>(args.0, args.1)
                .await
                .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
        },
    )?;
    rpc.register_async_method("ledger_getEventByNumber", |params, ledger| async move {
        let args: u64 = params.one()?;
        ledger
            .get_event_by_number::<E>(args)
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
    })?;

    rpc.register_async_method("ledger_getEventsByTxnHash", |params, ledger| async move {
        let args: HexHash = params.one()?;
        ledger
            .get_events_by_txn_hash::<E>(&args.0)
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
    })?;

    rpc.register_async_method("ledger_getEventsByKey", move |params, ledger| async move {
        let params: (&str, TxnRangeParam, usize, Option<&str>) = params.parse()?;
        ledger
            .get_events_by_key::<E>(params.0, params.1, params.2, params.3)
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
    })?;

    // Range getters.
    rpc.register_async_method("ledger_getSlotsRange", |params, ledger| async move {
        let args: RangeArgs = params.parse()?;
        ledger
            .get_slots_range::<B, Tx>(args.0, args.1, args.2)
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
    })?;
    rpc.register_async_method("ledger_getBatchesRange", |params, ledger| async move {
        let args: RangeArgs = params.parse()?;
        ledger
            .get_batches_range::<B, Tx>(args.0, args.1, args.2)
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
    })?;
    rpc.register_async_method("ledger_getTransactionsRange", |params, ledger| async move {
        let args: RangeArgs = params.parse()?;
        ledger
            .get_transactions_range::<Tx>(args.0, args.1, args.2)
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
    })?;

    rpc.register_async_method(
        "ledger_getAggregatedProofInfo",
        |_params, ledger| async move {
            let aggregated_proof = ledger
                .get_latest_aggregated_proof()
                .await
                .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))?;

            let info = aggregated_proof.map(|resp| {
                let public_data = resp.proof.public_data();
                ProofInfoResponse {
                    initial_slot_number: public_data.initial_slot_number,
                    final_slot_number: public_data.final_slot_number,
                }
            });

            Ok::<_, ErrorObjectOwned>(info)
        },
    )?;

    rpc.register_async_method("ledger_getAggregatedProof", |_params, ledger| async move {
        ledger
            .get_latest_aggregated_proof()
            .await
            .map_err(|e| to_jsonrpsee_error_object(e, LEDGER_RPC_ERROR))
    })?;

    rpc.register_subscription(
        "ledger_subscribeSlots",
        "ledger_slotProcessed",
        "ledger_unsubscribeSlots",
        |_, pending_subscription, db| async move {
            let rx = db.subscribe_slots();
            Ok(register_subscription::<u64>(pending_subscription, rx).await?)
        },
    )?;

    rpc.register_subscription(
        "ledger_subscribeAggregatedProof",
        "ledger_aggregatedProofProcessed",
        "ledger_unsubscribeAggregatedProof",
        |_, pending_subscription, db| async move {
            let rx = db.subscribe_proof_saved();
            Ok(register_subscription::<AggregatedProofResponse>(pending_subscription, rx).await?)
        },
    )?;

    Ok(rpc)
}

async fn register_subscription<T: Clone + Serialize>(
    pending_subscription: PendingSubscriptionSink,
    mut rx: Receiver<T>,
) -> Result<(), anyhow::Error> {
    // Accept the subscription. This message is sent immediately
    let subscription = pending_subscription.accept().await?;
    let closed = subscription.closed();
    futures::pin_mut!(closed);

    // This loop continues running until the subscription ends.
    loop {
        let next_msg = rx.recv();
        futures::pin_mut!(next_msg);
        match futures::future::select(closed, next_msg).await {
            // If the subscription closed, we're done
            Either::Left(_) => break Ok(()),
            // Otherwise, we need to send the message
            Either::Right((outcome, channel_closing_future)) => {
                let msg = SubscriptionMessage::from_json(&outcome?)?;
                // Sending only fails if the subscriber has canceled, so we can stop sending messages
                if subscription.send(msg).await.is_err() {
                    break Ok(());
                }
                closed = channel_closing_future;
            }
        }
    }
}

#[derive(serde::Deserialize)]
struct RangeArgs(u64, u64, #[serde(default)] QueryMode);

/// A structure containing serialized query arguments for RPC queries.
#[derive(serde::Deserialize)]
struct QueryArgs<T>(T, #[serde(default)] QueryMode);

/// Extract the args from an RPC query, being liberal in what is accepted.
/// To query for a list of items, users can either pass a list of ids, or tuple containing a list of ids and a query mode
fn extract_query_args<T: DeserializeOwned>(
    params: jsonrpsee::types::Params,
) -> Result<QueryArgs<T>, ErrorObjectOwned> {
    if let Ok(args) = params.parse() {
        return Ok(args);
    }
    let ids: T = params.parse()?;
    Ok(QueryArgs(ids, Default::default()))
}
