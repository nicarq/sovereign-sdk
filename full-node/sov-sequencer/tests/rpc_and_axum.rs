use base64::prelude::*;
use borsh::BorshSerialize;
use sov_mock_da::MockDaSpec;
use sov_modules_api::digest::Digest;
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{CryptoSpec, Spec};
use sov_rollup_interface::services::batch_builder::TxHash;
use sov_sequencer::utils::SimpleClient;
use sov_sequencer::TxStatus;
use sov_sequencer_json_client::types::PublishBatchBody;
use sov_test_utils::bank_data::BankMessageGenerator;
use sov_test_utils::runtime::TestRuntime;
use sov_test_utils::{MessageGenerator, TestPrivateKey, TestSpec};
use tempfile::TempDir;

/// Generates a hanful of transactions and returns the hash of the first one.
fn generate_txs(admin_private_key: TestPrivateKey) -> (TxHash, Vec<Transaction<TestSpec>>) {
    let bank_generator = BankMessageGenerator::<TestSpec>::with_minter(admin_private_key);
    let messages_iter = bank_generator.create_messages().into_iter();
    let mut txs = Vec::default();
    for message in messages_iter {
        let tx = message.to_tx::<TestRuntime<TestSpec, MockDaSpec>>();
        txs.push(tx);
    }

    let tx_hash: TxHash = <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::Hasher::digest(
        txs[0].try_to_vec().unwrap(),
    )
    .into();

    (tx_hash, txs)
}

#[tokio::test]
async fn rpc_subscribe() {
    let temp_dir = TempDir::new().unwrap();
    let setup = sov_test_utils::sequencer::new_sequencer(&temp_dir)
        .await
        .unwrap();
    let client = SimpleClient::new("127.0.0.1", setup.rpc_addr.port())
        .await
        .unwrap();

    let (tx_hash, txs) = generate_txs(setup.admin_private_key.clone());

    let mut subscription = client
        .subscribe_to_tx_status_updates::<()>(tx_hash)
        .await
        .unwrap();

    // Before submitting a transaction, its status is unknown.
    assert_eq!(
        subscription.next().await.unwrap().unwrap(),
        TxStatus::Unknown
    );

    client.send_transactions(&txs).await.unwrap();

    // The transaction status should change once it enters the mempool...
    assert_eq!(
        subscription.next().await.unwrap().unwrap(),
        TxStatus::Submitted
    );

    // ...and then change again shortly after, once it gets included in a block.
    assert_eq!(
        subscription.next().await.unwrap().unwrap(),
        TxStatus::Published {
            da_transaction_id: ()
        }
    );

    subscription.unsubscribe().await.unwrap();
}
#[tokio::test]
async fn axum_submit_batch_ok() {
    let temp_dir = TempDir::new().unwrap();
    let setup = sov_test_utils::sequencer::new_sequencer(&temp_dir)
        .await
        .unwrap();
    let client = sov_sequencer_json_client::Client::new(&format!(
        "http://127.0.0.1:{}",
        setup.axum_addr.port()
    ));

    let txs = generate_txs(setup.admin_private_key.clone()).1;

    let response = client
        .publish_batch(&PublishBatchBody {
            transactions: txs
                .iter()
                .map(|tx| BASE64_STANDARD.encode(tx.try_to_vec().unwrap()))
                .collect(),
        })
        .await
        .unwrap();

    assert_eq!(response.data.da_height, 0);
    assert_eq!(response.data.num_txs, txs.len() as i32);
}
