//! Basic request-response tests based on the ledger data supplied by
//! [`sov_test_utils::ledger_db::add_data_to_ledger_db`].

use std::str::FromStr;

use base64::prelude::*;
use sov_ledger_json_client::types::{AggregatedProof, Batch, Hash, IntOrHash, Tx};

use crate::common::LedgerTestService;

mod common;

/// We want 404s to return rich, JSON errors, like all the other kind of errors
/// we generate.
#[tokio::test(flavor = "multi_thread")]
async fn undefined_path_returns_json_error() {
    let ledger_service = LedgerTestService::new().await.unwrap();

    let addr = ledger_service.axum_handle.listening().await.unwrap();
    let response = reqwest::get(format!("http://{}/foobar", addr))
        .await
        .unwrap();
    assert_eq!(response.status(), 404);

    let response_body = response.text().await.unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&response_body).is_ok());
}

/// Asserts basic properties about the latest slot based on the data supplied by
/// [`sov_test_utils::ledger_db::add_data_to_ledger_db`].
#[tokio::test(flavor = "multi_thread")]
async fn get_latest_slot_is_ok() {
    let ledger_service = LedgerTestService::new().await.unwrap();
    let client = ledger_service.axum_client;

    let response = client.get_latest_slot().await.unwrap();
    assert_eq!(response.status(), 200);

    assert_eq!(response.data.number, 0);
    assert_eq!(response.data.batch_range.start, 0);
    assert_eq!(response.data.batch_range.end, 1);
    assert_eq!(response.data.batches, vec![]);

    // Moreover, fetching the latest slot by its ID should return the very same
    // slot data.
    assert_eq!(
        client.get_latest_slot().await.unwrap().data,
        client
            .get_slot_by_id(&IntOrHash::Variant0(0))
            .await
            .unwrap()
            .data,
    );
}

/// Getting a batch by number, hash, or slot offset should return the same
/// batch data.
#[tokio::test(flavor = "multi_thread")]
async fn get_batch() {
    fn assert_batch(batch_data: &Batch) {
        assert_eq!(batch_data.number, 0);
        assert_eq!(batch_data.tx_range.start, 0);
        assert_eq!(batch_data.tx_range.end, 1);
        assert_eq!(batch_data.txs, vec![]);
    }

    let ledger_service = LedgerTestService::new().await.unwrap();
    let client = ledger_service.axum_client;

    // By number.
    let response = client
        .get_batch_by_id(&IntOrHash::Variant0(0))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_batch(&response.data);

    // By hash.
    let hash = response.data.hash.clone();
    let response = client
        .get_batch_by_id(&IntOrHash::Variant1(
            Hash::from_str(&hash.to_string()).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_batch(&response.data);

    // By slot offset.
    let response = client
        .get_batch_by_slot_id_and_offset(&IntOrHash::Variant0(0), 0)
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_batch(&response.data);
}

/// All ways of getting a transaction should return the same transaction data.
#[tokio::test(flavor = "multi_thread")]
async fn get_tx() {
    fn assert_tx(tx_data: &Tx) {
        assert_eq!(tx_data.number, 0);
        assert_eq!(tx_data.event_range.start, 0);
        assert_eq!(tx_data.event_range.end, 2);
        assert_eq!(BASE64_STANDARD.decode(&tx_data.body).unwrap(), b"tx-body");
    }

    let ledger_service = LedgerTestService::new().await.unwrap();
    let client = ledger_service.axum_client;

    // By number.
    let response = client.get_tx_by_id(&IntOrHash::Variant0(0)).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_tx(&response.data);

    // By hash.
    let hash = response.data.hash.clone();
    let response = client
        .get_tx_by_id(&IntOrHash::Variant1(
            Hash::from_str(&hash.to_string()).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_tx(&response.data);

    // By slot offset.
    let response = client
        .get_tx_by_slot_id_and_offset(&IntOrHash::Variant0(0), 0, 0)
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_tx(&response.data);

    // By batch offset.
    let response = client
        .get_tx_by_batch_id_and_offset(&IntOrHash::Variant0(0), 0)
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_tx(&response.data);
}

#[tokio::test(flavor = "multi_thread")]
async fn get_event() {
    let ledger_service = LedgerTestService::new().await.unwrap();
    let client = ledger_service.axum_client;

    let response = client.get_event_by_id(0).await.unwrap();
    let expected_value = r#"
{
    "TokenCreated": {
        "authorized_minters": [],
        "coins": {
            "amount": 0,
            "token_id": "token_1rwrh8gn2py0dl4vv65twgctmlwck6esm2as9dftumcw89kqqn3nqrduss6"
        },
        "minter": {
            "Module": "module_1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqn7stmj"
        },
        "token_name": "token"
    }
}
"#;
    assert_eq!(response.status(), 200);
    assert_eq!(response.data.key, "foo");
    assert_eq!(
        response.data.value,
        serde_json::from_str(expected_value).unwrap()
    );
    assert_eq!(response.data.module.name, "bank");
}

#[tokio::test(flavor = "multi_thread")]
async fn get_latest_aggregated_proof() {
    fn assert_aggregated_proof(aggregated_proof: &AggregatedProof) {
        assert_eq!(aggregated_proof.public_data.initial_slot_number, u64::MAX);
        assert_eq!(aggregated_proof.public_data.final_slot_number, u64::MAX);
        assert_eq!(
            BASE64_STANDARD
                .decode(&aggregated_proof.public_data.genesis_state_root)
                .unwrap(),
            b"genesis-state-root".to_vec()
        );
        assert_eq!(
            BASE64_STANDARD
                .decode(&aggregated_proof.public_data.initial_state_root)
                .unwrap(),
            b"initial-state-root".to_vec()
        );
        assert_eq!(
            BASE64_STANDARD
                .decode(&aggregated_proof.public_data.final_state_root)
                .unwrap(),
            b"final-state-root".to_vec()
        );
        assert_eq!(
            BASE64_STANDARD
                .decode(&aggregated_proof.public_data.initial_slot_hash)
                .unwrap(),
            b"initial-slot-hash".to_vec()
        );
        assert_eq!(
            BASE64_STANDARD
                .decode(&aggregated_proof.public_data.final_slot_hash)
                .unwrap(),
            b"final-slot-hash".to_vec()
        );
        assert_eq!(
            BASE64_STANDARD
                .decode(&aggregated_proof.public_data.code_commitment.0)
                .unwrap(),
            b"code-commitment".to_vec()
        );
    }

    let ledger_service = LedgerTestService::new().await.unwrap();
    let client = ledger_service.axum_client;

    let response = client.get_latest_aggregated_proof().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_aggregated_proof(&response.data);
}
