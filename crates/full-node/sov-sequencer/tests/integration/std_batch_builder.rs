use std::time::Duration;

use base64::prelude::*;
use borsh::BorshDeserialize;
use sov_kernels::basic::BasicKernel;
use sov_mock_da::{MockDaService, MockDaSpec};
use sov_modules_api::capabilities::TransactionAuthenticator;
use sov_modules_api::prelude::*;
use sov_modules_api::transaction::{Transaction, UnsignedTransaction};
use sov_modules_api::{Batch, BlobReaderTrait, FullyBakedTx, RawTx};
use sov_rollup_interface::node::da::DaService;
use sov_sequencer::batch_builders::standard::{StdBatchBuilder, StdBatchBuilderConfig};
use sov_sequencer_json_client::types;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::TestOptimisticRuntime;
use sov_test_utils::sequencer::TestSequencerSetup;
use sov_test_utils::{EncodeCall, TestSpec, TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE};
use sov_value_setter::ValueSetter;

pub type MyBatchBuilder = StdBatchBuilder<
    (
        TestSpec,
        MockDaSpec,
        TestOptimisticRuntime<TestSpec, MockDaSpec>,
    ),
    BasicKernel<TestSpec, MockDaSpec>,
>;

async fn new_sequencer() -> TestSequencerSetup<MyBatchBuilder> {
    let dir = tempfile::tempdir().unwrap();
    let da_service = MockDaService::new(HighLevelOptimisticGenesisConfig::SEQUENCER_DA_ADDR);

    let batch_builder_config = StdBatchBuilderConfig {
        mempool_max_txs_count: None,
        max_batch_size_bytes: None,
    };

    TestSequencerSetup::new(dir, da_service, batch_builder_config, vec![])
        .await
        .unwrap()
}

fn valid_tx_bytes(setup: &TestSequencerSetup<MyBatchBuilder>, nonce: u64) -> RawTx {
    let msg = <TestOptimisticRuntime<TestSpec, MockDaSpec> as EncodeCall<ValueSetter<TestSpec>>>::encode_call(
        sov_value_setter::CallMessage::SetValue(1),
    );

    let tx = borsh::to_vec(&Transaction::<TestSpec>::new_signed_tx(
        &setup.admin_private_key,
        UnsignedTransaction::new(
            msg,
            config_value!("CHAIN_ID"),
            TEST_DEFAULT_MAX_PRIORITY_FEE,
            TEST_DEFAULT_MAX_FEE,
            nonce,
            None,
        ),
    ))
    .unwrap();

    RawTx::new(tx)
}

fn wrap_with_auth(raw_tx: RawTx) -> FullyBakedTx {
    TestOptimisticRuntime::<TestSpec, MockDaSpec>::encode_with_standard_auth(raw_tx)
}

#[tokio::test]
#[traced_test]
async fn dropping_sequencer_stops_listener() {
    let sequencer = new_sequencer().await;

    assert!(!logs_contain("stopping listener"));

    drop(sequencer);
    tokio::time::sleep(Duration::from_millis(20)).await;

    assert!(logs_contain("stopping listener"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_submit_on_empty_mempool() {
    let sequencer = new_sequencer().await;
    let client = sequencer.client();

    let error_response = client
        .publish_batch(&types::PublishBatchBody {
            transactions: vec![],
        })
        .await
        .unwrap_err();

    dbg!(&error_response);
    assert_eq!(error_response.status().map(|s| s.as_u16()), Some(409));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_submit_happy_path() {
    let sequencer = new_sequencer().await;
    let tx1 = valid_tx_bytes(&sequencer, 0);
    let tx2 = valid_tx_bytes(&sequencer, 1);
    sequencer
        .client()
        .accept_tx(&types::AcceptTxBody {
            body: BASE64_STANDARD.encode(&tx1),
        })
        .await
        .unwrap();

    sequencer
        .client()
        .accept_tx(&types::AcceptTxBody {
            body: BASE64_STANDARD.encode(&tx2),
        })
        .await
        .unwrap();

    sequencer
        .client()
        .publish_batch(&types::PublishBatchBody {
            transactions: vec![],
        })
        .await
        .unwrap();

    let mut submitted_block = sequencer.da_service.get_block_at(1).await.unwrap();
    let block_data = submitted_block.batch_blobs[0].full_data();

    let batch = Batch::try_from_slice(block_data).unwrap();

    assert_eq!(batch.txs.len(), 2);
    assert_eq!(wrap_with_auth(tx1), batch.txs[0]);
    assert_eq!(wrap_with_auth(tx2), batch.txs[1]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_accept_tx() {
    let sequencer = new_sequencer().await;

    let client = sequencer.client();

    let tx = valid_tx_bytes(&sequencer, 0);

    client
        .accept_tx(&types::AcceptTxBody {
            body: BASE64_STANDARD.encode(&tx.data),
        })
        .await
        .unwrap();

    client
        .publish_batch(&types::PublishBatchBody {
            transactions: vec![],
        })
        .await
        .unwrap();

    let mut submitted_block = sequencer.da_service.get_block_at(1).await.unwrap();
    let block_data = submitted_block.batch_blobs[0].full_data();

    let batch = Batch::try_from_slice(block_data).unwrap();

    assert_eq!(wrap_with_auth(tx).data, batch.txs[0].data);
}
