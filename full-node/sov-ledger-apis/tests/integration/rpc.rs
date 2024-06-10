use jsonrpsee::core::client::ClientT;
use jsonrpsee::core::params::ArrayParams;
use sov_ledger_apis::rpc::client::RpcClient;
use sov_ledger_apis::HexHash;
use sov_modules_api::StoredEvent;
use sov_rollup_interface::rpc::{EventIdentifier, QueryMode, TxIdAndOffset, TxIdentifier};
use sov_test_utils::ledger_db::LedgerTestService;

#[tokio::test(flavor = "multi_thread")]
async fn getters_succeed() {
    let ledger_service = LedgerTestService::new().await.unwrap();
    let rpc_client = ledger_service.rpc_client().await;

    rpc_client.get_head(QueryMode::Compact).await.unwrap();
    rpc_client.get_head(QueryMode::Standard).await.unwrap();
    rpc_client.get_head(QueryMode::Full).await.unwrap();

    rpc_client
        .get_slots(vec![], QueryMode::Compact)
        .await
        .unwrap();
    rpc_client
        .get_batches(vec![], QueryMode::Compact)
        .await
        .unwrap();
    rpc_client
        .get_transactions(vec![], QueryMode::Compact)
        .await
        .unwrap();
    rpc_client.get_events(vec![]).await.unwrap();

    let hash = HexHash([0; 32]);
    rpc_client
        .get_slot_by_hash(hash, QueryMode::Compact)
        .await
        .unwrap();
    rpc_client
        .get_batch_by_hash(hash, QueryMode::Compact)
        .await
        .unwrap();
    rpc_client
        .get_tx_by_hash(hash, QueryMode::Compact)
        .await
        .unwrap();

    rpc_client
        .get_slot_by_number(0, QueryMode::Compact)
        .await
        .unwrap();
    rpc_client
        .get_batch_by_number(0, QueryMode::Compact)
        .await
        .unwrap();
    rpc_client
        .get_tx_by_number(0, QueryMode::Compact)
        .await
        .unwrap();
    rpc_client
        .get_tx_numbers_by_hash(hash, QueryMode::Compact)
        .await
        .unwrap();
    rpc_client
        .get_slots_range(0, 1, QueryMode::Compact)
        .await
        .unwrap();
    rpc_client
        .get_batches_range(0, 1, QueryMode::Compact)
        .await
        .unwrap();
    rpc_client
        .get_txs_range(0, 1, QueryMode::Compact)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn subscribe_slots_succeeds() {
    let ledger_service = LedgerTestService::new().await.unwrap();
    let rpc_client = ledger_service.rpc_client().await;

    rpc_client.subscribe_slots().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn get_head_with_optional_query_mode() {
    let ledger_service = LedgerTestService::new().await.unwrap();
    let rpc_client = ledger_service.rpc_client().await;

    // No QueryMode param.
    {
        rpc_client
            .request::<serde_json::Value, _>("ledger_getHead", ArrayParams::new())
            .await
            .unwrap();
    }
    // With QueryMode param.
    {
        let mut params = ArrayParams::new();
        params.insert(QueryMode::Standard).unwrap();
        rpc_client
            .request::<serde_json::Value, _>("ledger_getHead", params)
            .await
            .unwrap();
    }
}

/// `ledger_getEvents` supports several parameter types, because of a
/// `jsonrpsee` limitation. See:
/// - https://github.com/Sovereign-Labs/sovereign-sdk/pull/1058
/// - https://github.com/Sovereign-Labs/sovereign-sdk/issues/1037
///
/// While `jsonrpsee` macro-generated clients can only generate nested array
/// types as parameters (e.g. `"params": [[1, 2, 3]]`), we want to test that
/// non-nested array types are also supported (e.g. `"params": [1, 2, 3]` and
/// `"params": [{"txId": 1, "offset": 2}]`).
#[tokio::test(flavor = "multi_thread")]
async fn get_events_patterns() {
    let ledger_service = LedgerTestService::new().await.unwrap();
    let rpc_client = ledger_service.rpc_client().await;

    rpc_client
        .get_events(vec![EventIdentifier::Number(2)])
        .await
        .unwrap();
    rpc_client
        .request::<Vec<Option<StoredEvent>>, _>("ledger_getEvents", vec![vec![2]])
        .await
        .unwrap();
    rpc_client
        .request::<Vec<Option<StoredEvent>>, _>("ledger_getEvents", vec![2])
        .await
        .unwrap();
    rpc_client
        .request::<Vec<Option<StoredEvent>>, _>(
            "ledger_getEvents",
            vec![EventIdentifier::TxIdAndOffset(TxIdAndOffset {
                tx_id: TxIdentifier::Number(1),
                offset: 2,
            })],
        )
        .await
        .unwrap();
}
