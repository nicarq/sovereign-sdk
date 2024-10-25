use base64::prelude::*;
use sov_api_spec::types::PublishBatchBody;
use sov_test_utils::sequencer::TestSequencerSetup;

use crate::utils::generate_txs;

#[tokio::test(flavor = "multi_thread")]
async fn axum_submit_batch_ok() {
    let sequencer = TestSequencerSetup::with_real_batch_builder().await.unwrap();
    let client = sequencer.client();

    let txs = generate_txs(sequencer.admin_private_key.clone());

    let response_result = client
        .publish_batch(&PublishBatchBody {
            transactions: txs
                .iter()
                .map(|tx| BASE64_STANDARD.encode(&tx.raw_tx.data))
                .collect(),
        })
        .await;

    let response_data = &response_result.unwrap().data.clone().unwrap();

    assert_eq!(response_data.da_height, 0);
    assert_eq!(response_data.num_txs, txs.len() as i32);
}
