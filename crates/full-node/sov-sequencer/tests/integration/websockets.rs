use futures::stream::StreamExt;
use sov_api_spec::types::TxStatus;
use sov_test_utils::sequencer::TestSequencerSetup;

use crate::utils::generate_txs;

#[tokio::test(flavor = "multi_thread")]
async fn mempool_eviction_event() {
    let mempool_max_txs_count = 1;
    let sequencer = TestSequencerSetup::with_real_batch_builder_and_mempool_max_txs_count(
        mempool_max_txs_count.try_into().unwrap(),
    )
    .await
    .unwrap();

    let txs = generate_txs(sequencer.admin_private_key.clone());

    let client = sequencer.client();
    let mut subscription = client
        .subscribe_to_tx_status_updates(txs[0].tx_hash)
        .await
        .unwrap();

    // Before submitting a transaction, its status is unknown.
    assert_eq!(
        subscription.next().await.unwrap().unwrap().status,
        TxStatus::Unknown
    );

    sequencer
        .sequencer
        .accept_tx(txs[0].tx_input.clone())
        .await
        .unwrap();

    // The transaction enters the mempool.
    assert_eq!(
        subscription.next().await.unwrap().unwrap().status,
        TxStatus::Submitted
    );

    // In the meantime, another transaction enters the mempool and causes the
    // first one to be evicted.
    sequencer
        .sequencer
        .accept_tx(txs[1].tx_input.clone())
        .await
        .unwrap();

    assert_eq!(
        subscription.next().await.unwrap().unwrap().status,
        TxStatus::Dropped,
    );

    // No more events.
    assert!(subscription.next().await.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_subscribe() {
    let sequencer = TestSequencerSetup::with_real_batch_builder().await.unwrap();
    let client = sequencer.client();

    let txs = generate_txs(sequencer.admin_private_key.clone());

    let mut subscription = client
        .subscribe_to_tx_status_updates(txs[0].tx_hash)
        .await
        .unwrap();

    // Before submitting a transaction, its status is unknown.
    assert_eq!(
        subscription.next().await.unwrap().unwrap().status,
        TxStatus::Unknown
    );

    client
        .publish_batch_with_serialized_txs(&[txs[0].tx_object.clone()])
        .await
        .unwrap();

    // The transaction status should change once it enters the mempool...
    assert_eq!(
        subscription.next().await.unwrap().unwrap().status,
        TxStatus::Submitted
    );

    // ...and then change again shortly after, once it gets included in a block.
    assert_eq!(
        subscription.next().await.unwrap().unwrap().status,
        TxStatus::Published,
    );

    // TODO: finalized status (https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1088).

    // No more events.
    //assert!(subscription.next().await.is_none());
}
