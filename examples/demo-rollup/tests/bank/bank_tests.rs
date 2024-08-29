use anyhow::Context;
use futures::StreamExt;
use sov_bank::event::Event as BankEvent;
use sov_bank::utils::TokenHolder;
use sov_bank::Coins;
use sov_mock_da::storable::service::StorableMockDaService;
use sov_mock_da::BlockProducingConfig;
use sov_rollup_interface::node::da::DaServiceWithRetries;
use sov_rollup_interface::node::ledger_api::FinalityStatus;
use sov_test_utils::{ApiClient, TestSpec};

use super::helpers::*;
use super::TxSender;
use crate::bank::{DaLayerTxSender, SequencerTxSender, TOKEN_NAME, TOKEN_SALT};
use crate::test_helpers::get_appropriate_rollup_prover_config;

const BLOCK_PRODUCING_CONFIG: BlockProducingConfig = BlockProducingConfig::OnSubmit;

#[tokio::test(flavor = "multi_thread")]
async fn bank_tx_tests_instant_finality_using_sequencer_tx_submission() -> anyhow::Result<()> {
    let test_case = TestCase {
        wait_for_aggregated_proof: true,
        finalization_blocks: 0,
    };

    let test_rollup = TestRollup::create_test_rollup(
        &test_case,
        get_appropriate_rollup_prover_config(),
        BLOCK_PRODUCING_CONFIG,
    )
    .await?;

    let sender = SequencerTxSender {};

    // If the rollup throws an error, return it and stop trying to send the transaction
    tokio::select! {
        err = test_rollup.rollup_task => err?,
        res = send_test_bank_txs(test_case, &test_rollup.client, &test_rollup.da_service, sender) => res?,
    };

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn bank_tx_tests_non_instant_finality_using_sequencer_tx_submission() -> anyhow::Result<()> {
    let test_case = TestCase {
        wait_for_aggregated_proof: false,
        finalization_blocks: 2,
    };
    let test_rollup = TestRollup::create_test_rollup(
        &test_case,
        get_appropriate_rollup_prover_config(),
        BLOCK_PRODUCING_CONFIG,
    )
    .await?;

    let sender = SequencerTxSender {};

    // If the rollup throws an error, return it and stop trying to send the transaction
    tokio::select! {
        err = test_rollup.rollup_task => err?,
        res = send_test_bank_txs(test_case, &test_rollup.client, &test_rollup.da_service, sender) => res?,
    };

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn bank_tx_tests_instant_finality_using_da_layer_tx_submission() -> anyhow::Result<()> {
    let test_case = TestCase {
        wait_for_aggregated_proof: true,
        finalization_blocks: 0,
    };

    let test_rollup = TestRollup::create_test_rollup(
        &test_case,
        get_appropriate_rollup_prover_config(),
        BLOCK_PRODUCING_CONFIG,
    )
    .await?;

    let sender = DaLayerTxSender::new(test_rollup.da_service.clone());
    // If the rollup throws an error, return it and stop trying to send the transaction
    tokio::select! {
        err = test_rollup.rollup_task => err?,
        res = send_test_bank_txs(test_case, &test_rollup.client, &test_rollup.da_service, sender) => res?,
    };

    Ok(())
}

async fn send_test_bank_txs(
    test_case: TestCase,
    client: &ApiClient,
    da_service: &DaServiceWithRetries<StorableMockDaService>,
    tx_sender: impl TxSender,
) -> anyhow::Result<()> {
    let (key, user_address, token_id, recipient_address) = create_keys_and_addresses();
    let token_id_response = sov_bank::BankRpcClient::<TestSpec>::token_id(
        &client.rpc,
        TOKEN_NAME.to_owned(),
        user_address,
        TOKEN_SALT,
    )
    .await?;

    let mut aggrgeated_proofs_posted_to_da_subscription =
        da_service.da_service().subscribe_proof_posted();

    let mut aggregated_proof_subscription = client
        .ledger
        .subscribe_aggregated_proof()
        .await
        .context("Failed to subscribe to aggregated proof")?;

    assert_eq!(token_id, token_id_response);

    // create token. height 2
    let tx = build_create_token_tx(&key, 0);
    let slot_number = tx_sender.send_txs(client, &[tx]).await?;
    assert_eq!(1, slot_number);
    assert_slot_finality(client, slot_number, test_case.expected_head_finality()).await;

    assert_balance(client, 1000, token_id, user_address, None).await?;

    // transfer 100 tokens. assert sender balance. height 3
    let tx = build_transfer_token_tx(&key, token_id, recipient_address, 100, 1);
    let slot_number = tx_sender.send_txs(client, &[tx]).await?;
    assert_eq!(2, slot_number);
    assert_slot_finality(client, slot_number, test_case.expected_head_finality()).await;

    if test_case.wait_for_aggregated_proof {
        aggrgeated_proofs_posted_to_da_subscription
            .recv()
            .await
            .unwrap();
    }

    assert_balance(client, 900, token_id, user_address, None).await?;
    // transfer 200 tokens. assert sender balance. height 4
    let tx = build_transfer_token_tx(&key, token_id, recipient_address, 200, 2);
    let slot_number = tx_sender.send_txs(client, &[tx]).await?;
    assert_eq!(3, slot_number);
    assert_slot_finality(client, slot_number, test_case.expected_head_finality()).await;

    assert_balance(client, 700, token_id, user_address, None).await?;

    // assert sender balance at height 2.
    assert_balance(client, 1000, token_id, user_address, Some(2)).await?;

    // assert sender balance at height 3.
    assert_balance(client, 900, token_id, user_address, Some(3)).await?;

    // assert sender balance at height 4.
    assert_balance(client, 700, token_id, user_address, Some(4)).await?;

    // 10 transfers of 10,11..20
    let transfer_amounts: Vec<u64> = (10u64..20).collect();
    let txs = build_multiple_transfers(&transfer_amounts, &key, token_id, recipient_address, 3);
    let slot_number = tx_sender.send_txs(client, &txs).await?;
    assert_eq!(4, slot_number);
    assert_slot_finality(client, slot_number, test_case.expected_head_finality()).await;

    assert_bank_event::<TestSpec>(
        client,
        0,
        BankEvent::TokenCreated {
            token_name: TOKEN_NAME.to_owned(),
            coins: Coins {
                amount: 1000,
                token_id,
            },
            minter: TokenHolder::User(user_address),
            authorized_minters: vec![],
        },
    )
    .await?;
    assert_bank_event::<TestSpec>(
        client,
        1,
        BankEvent::TokenTransferred {
            from: TokenHolder::User(user_address),
            to: TokenHolder::User(recipient_address),
            coins: Coins {
                amount: 100,
                token_id,
            },
        },
    )
    .await?;
    assert_bank_event::<TestSpec>(
        client,
        2,
        BankEvent::TokenTransferred {
            from: TokenHolder::User(user_address),
            to: TokenHolder::User(recipient_address),
            coins: Coins {
                amount: 200,
                token_id,
            },
        },
    )
    .await?;

    if test_case.wait_for_aggregated_proof {
        let aggregated_proof_resp = aggregated_proof_subscription.next().await.unwrap()?;
        let pub_data = aggregated_proof_resp.public_data;
        assert_aggregated_proof_public_data(1, 1, &pub_data);
        assert_aggregated_proof(1, 1, client).await?;
    }

    if let Some(finalized_slot_number) = test_case.get_latest_finalized_slot_after(slot_number) {
        assert_slot_finality(client, finalized_slot_number, FinalityStatus::Finalized).await;
    }

    Ok(())
}
