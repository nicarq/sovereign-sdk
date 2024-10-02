use std::str::FromStr;
use std::time::Duration;

use base64::prelude::*;
use borsh::BorshDeserialize;
use sov_kernels::basic::BasicKernel;
use sov_mock_da::{MockDaService, MockDaSpec};
use sov_modules_api::capabilities::TransactionAuthenticator;
use sov_modules_api::prelude::*;
use sov_modules_api::transaction::{Transaction, UnsignedTransaction};
use sov_modules_api::{Address, Batch, BlobReaderTrait, FullyBakedTx, RawTx};
use sov_rollup_interface::node::da::DaService;
use sov_sequencer::batch_builders::standard::{StdBatchBuilder, StdBatchBuilderConfig};
use sov_sequencer_json_client::types;
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::{config_gas_token_id, Bank, Coins, TestOptimisticRuntime};
use sov_test_utils::sequencer::TestSequencerSetup;
use sov_test_utils::{
    EncodeCall, TestSpec, TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE,
    TEST_DEFAULT_USER_BALANCE,
};
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

fn build_tx(
    setup: &TestSequencerSetup<MyBatchBuilder>,
    nonce: u64,
    call_message: Vec<u8>,
) -> RawTx {
    let tx = borsh::to_vec(&Transaction::<TestSpec>::new_signed_tx(
        &setup.admin_private_key,
        UnsignedTransaction::new(
            call_message,
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

fn valid_tx_bytes(
    setup: &TestSequencerSetup<MyBatchBuilder>,
    nonce: u64,
    value_to_set: u32,
) -> RawTx {
    let msg = <TestOptimisticRuntime<TestSpec, MockDaSpec> as EncodeCall<ValueSetter<TestSpec>>>::encode_call(
        sov_value_setter::CallMessage::SetValue(value_to_set),
    );

    build_tx(setup, nonce, msg)
}

fn wrap_with_auth(raw_tx: RawTx) -> FullyBakedTx {
    TestOptimisticRuntime::<TestSpec, MockDaSpec>::encode_with_standard_auth(raw_tx)
}

// This test has to be single-threaded because logs from other threads don't
// show up in traced_test (https://github.com/dbrgn/tracing-test/issues/23).
// This also means we have to be on tokio 1.41 or newer,
// to prevent an indefinite stall due to https://github.com/tokio-rs/tokio/issues/6839.
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
    let tx1 = valid_tx_bytes(&sequencer, 0, 0);
    let tx2 = valid_tx_bytes(&sequencer, 1, 1);
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

    let tx = valid_tx_bytes(&sequencer, 0, 0);

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

#[tokio::test(flavor = "multi_thread")]
// Test how the batch builder handles invalid transactions inside the mempool by
// inserting two valid transactions, then draining the sender's balance so that
// the second one has insufficent gas. This is a regression test for the bug
// where an out-of-gas error during batch creation prevented the entire
// batch from being submitted.
async fn test_batch_building_with_out_of_gas_error() {
    let sequencer = new_sequencer().await;

    // -- Build a transaction which drains the user's wallet --
    let drain_wallet_msg = sov_test_utils::runtime::BankCallMessage::<TestSpec>::Transfer {
        to: Address::from_str("sov1pv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9skzctpv9stup8tx")
            .unwrap(),
        coins: Coins {
            amount: TEST_DEFAULT_USER_BALANCE - TEST_DEFAULT_MAX_FEE, // Leave enough tokens to pay gas for the first tx
            token_id: config_gas_token_id(),
        },
    };
    let drain_wallet_msg: Vec<u8> = <TestOptimisticRuntime<TestSpec, MockDaSpec> as EncodeCall<
        Bank<TestSpec>,
    >>::encode_call(drain_wallet_msg);
    // --  END tx construction --

    let drainer = build_tx(&sequencer, 0, drain_wallet_msg);
    let tx_with_insufficient_gas = valid_tx_bytes(&sequencer, 1, 1);

    // Send the two transactions, drainer first. Since the default builder
    // uses FIFO, this ensures that it gets included first, leaving the other one out of gas.
    let client = sequencer.client();
    client
        .accept_tx(&types::AcceptTxBody {
            body: BASE64_STANDARD.encode(&drainer.data),
        })
        .await
        .unwrap();

    client
        .accept_tx(&types::AcceptTxBody {
            body: BASE64_STANDARD.encode(&tx_with_insufficient_gas.data),
        })
        .await
        .unwrap();

    client
        .publish_batch(&types::PublishBatchBody {
            transactions: vec![],
        })
        .await
        .unwrap();

    // As a sanity check, assert that the second transaction wasn't included
    let mut submitted_block = sequencer.da_service.get_block_at(1).await.unwrap();
    let block_data = submitted_block.batch_blobs[0].full_data();
    let batch = Batch::try_from_slice(block_data).unwrap();
    assert_eq!(wrap_with_auth(drainer).data, batch.txs[0].data);
    assert_eq!(batch.txs.len(), 1);
}
