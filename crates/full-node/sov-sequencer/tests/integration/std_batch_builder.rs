use std::str::FromStr;

use axum::http::StatusCode;
use base64::prelude::*;
use borsh::BorshDeserialize;
use sov_api_spec::types;
use sov_mock_da::MockDaService;
use sov_modules_api::prelude::*;
use sov_modules_api::{Address, BlobReaderTrait, DispatchCall, FullyBakedTx};
use sov_rollup_interface::node::da::DaService;
use sov_sequencer::batch_builders::standard::{StdBatchBuilder, StdBatchBuilderConfig};
use sov_test_utils::runtime::genesis::optimistic::HighLevelOptimisticGenesisConfig;
use sov_test_utils::runtime::{config_gas_token_id, Coins, TestOptimisticRuntime};
use sov_test_utils::sequencer::TestSequencerSetup;
use sov_test_utils::{TestSpec, TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_USER_BALANCE};

use crate::utils::{
    build_tx, generate_paymaster_tx, new_sequencer, valid_tx_bytes, wrap_with_auth,
};

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

    let batch = Vec::<FullyBakedTx>::try_from_slice(block_data).unwrap();

    assert_eq!(batch.len(), 2);
    assert_eq!(wrap_with_auth(tx1), batch[0]);
    assert_eq!(wrap_with_auth(tx2), batch[1]);
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
    let batch = Vec::<FullyBakedTx>::try_from_slice(block_data).unwrap();

    assert_eq!(wrap_with_auth(tx).data, batch[0].data);
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
    let drain_wallet_msg =
        <TestOptimisticRuntime<TestSpec> as DispatchCall>::Decodable::Bank(drain_wallet_msg);
    // --  END tx construction --

    let drainer = build_tx(&sequencer, 0, &drain_wallet_msg);
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
    let batch = Vec::<FullyBakedTx>::try_from_slice(block_data).unwrap();
    assert_eq!(wrap_with_auth(drainer).data, batch[0].data);
    assert_eq!(batch.len(), 1);
}

// Checks that transactions that are not sequencer safe are rejected
// when the sender address is not configured as an admin in the sequencer config.
#[tokio::test(flavor = "multi_thread")]
async fn not_sequencer_safe_txs_are_restricted() {
    let dir = tempfile::tempdir().unwrap();
    let sequencer_addr = HighLevelOptimisticGenesisConfig::SEQUENCER_DA_ADDR;
    let da_service = MockDaService::new(sequencer_addr);
    let batch_builder_config = StdBatchBuilderConfig {
        mempool_max_txs_count: None,
        max_batch_size_bytes: None,
    };

    let sequencer = TestSequencerSetup::<
        StdBatchBuilder<(TestSpec, TestOptimisticRuntime<TestSpec>)>,
    >::new(dir, da_service, batch_builder_config, false)
    .await
    .unwrap();

    let tx = generate_paymaster_tx(sequencer.admin_private_key.clone());
    {
        let client = sequencer.client();

        client
            .accept_tx(&types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
            .unwrap();
        if let Err(e) = client
            .publish_batch(&types::PublishBatchBody {
                transactions: vec![],
            })
            .await
        {
            assert!(
                e.status()
                    .is_some_and(|status| status == StatusCode::CONFLICT),
                "{e}"
            );
        } else {
            panic!("Sequencer accepted admin tx from non-admin sender");
        }
    }
}

// Checks that transactions that are not sequencer safe are accepted
// if the sender address is configured as an admin in the sequencer config.
#[tokio::test(flavor = "multi_thread")]
async fn sequencer_safe_txs_from_admins_are_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let sequencer_addr = HighLevelOptimisticGenesisConfig::SEQUENCER_DA_ADDR;
    let da_service = MockDaService::new(sequencer_addr);
    let batch_builder_config = StdBatchBuilderConfig {
        mempool_max_txs_count: None,
        max_batch_size_bytes: None,
    };

    let sequencer = TestSequencerSetup::<
        StdBatchBuilder<(TestSpec, TestOptimisticRuntime<TestSpec>)>,
    >::new(dir, da_service, batch_builder_config, true)
    .await
    .unwrap();

    let tx = generate_paymaster_tx(sequencer.admin_private_key.clone());
    {
        let client = sequencer.client();

        client
            .accept_tx(&types::AcceptTxBody {
                body: BASE64_STANDARD.encode(&tx),
            })
            .await
            .unwrap();

        client
            .publish_batch(&types::PublishBatchBody {
                transactions: vec![],
            })
            .await
            .expect("Batch publication should succeed because the provided tx is valid and comes from an admin");
    }
}
