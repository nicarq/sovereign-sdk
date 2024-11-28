use anyhow::Context;
use futures::StreamExt;
use sov_bank::event::Event as BankEvent;
use sov_bank::utils::TokenHolder;
use sov_bank::Coins;
use sov_cli::NodeClient;
use sov_demo_rollup::MockDemoRollup;
use sov_mock_da::storable::service::StorableMockDaService;
use sov_mock_da::BlockProducingConfig;
use sov_modules_api::execution_mode::Native;
use sov_modules_api::OperatingMode;
use sov_rollup_interface::node::da::DaServiceWithRetries;
use sov_rollup_interface::node::ledger_api::FinalityStatus;
use sov_test_utils::test_rollup::RollupBuilder;
use sov_test_utils::TestSpec;

use crate::bank::helpers::*;
use crate::bank::{SequencerTxSender, TxSender, TOKEN_NAME};
use crate::test_helpers::test_genesis_source;

const BLOCK_PRODUCING_CONFIG: BlockProducingConfig = BlockProducingConfig::OnBatchSubmit;

#[tokio::test(flavor = "multi_thread")]
async fn flaky_bank_tx_tests_instant_finality_using_sequencer_tx_submission() -> anyhow::Result<()>
{
    let test_case = TestCase {
        wait_for_aggregated_proof: true,
        finalization_blocks: 0,
    };

    let test_rollup = RollupBuilder::<MockDemoRollup<Native>>::new(
        test_genesis_source(OperatingMode::Zk),
        BLOCK_PRODUCING_CONFIG,
        test_case.finalization_blocks,
    )
    .start()
    .await?;

    let sender = SequencerTxSender {};

    // If the rollup throws an error, return it and stop trying to send the transaction
    tokio::select! {
        err = test_rollup.rollup_task => err?,
        res = send_test_bank_txs(test_case, &test_rollup.client, &test_rollup.da_service, sender) => Ok(res?),
    }?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn flaky_bank_tx_tests_non_instant_finality_using_sequencer_tx_submission(
) -> anyhow::Result<()> {
    let test_case = TestCase {
        wait_for_aggregated_proof: false,
        finalization_blocks: 2,
    };
    let test_rollup = RollupBuilder::<MockDemoRollup<Native>>::new(
        test_genesis_source(OperatingMode::Zk),
        BLOCK_PRODUCING_CONFIG,
        test_case.finalization_blocks,
    )
    .start()
    .await?;

    let sender = SequencerTxSender {};

    // If the rollup throws an error, return it and stop trying to send the transaction
    tokio::select! {
        err = test_rollup.rollup_task => err?,
        res = send_test_bank_txs(test_case, &test_rollup.client, &test_rollup.da_service, sender) => Ok(res?),
    }?;

    Ok(())
}

async fn send_test_bank_txs(
    test_case: TestCase,
    client: &NodeClient,
    da_service: &DaServiceWithRetries<StorableMockDaService>,
    tx_sender: impl TxSender,
) -> anyhow::Result<()> {
    let (key, user_address, token_id, recipient_address) = create_keys_and_addresses();
    let genesis_gas_balance = client
        .get_balance::<TestSpec>(&user_address, &sov_bank::config_gas_token_id(), Some(0))
        .await?;

    let token_id_response = client
        .get_token_id::<TestSpec>(TOKEN_NAME, &user_address)
        .await?;

    let mut aggregated_proofs_posted_to_da_subscription =
        da_service.da_service().subscribe_proof_posted();

    let mut aggregated_proof_subscription = client
        .client
        .subscribe_aggregated_proof()
        .await
        .context("Failed to subscribe to aggregated proof")?;

    assert_eq!(token_id, token_id_response);

    // create token. height 2
    let tx = build_create_token_tx(&key, 0, 1000);
    let rollup_height = tx_sender.send_txs(client, &[tx]).await?;
    assert_eq!(1, rollup_height);
    assert_slot_finality(client, rollup_height, test_case.expected_head_finality()).await;
    let gas_balance_height_1 = client
        .get_balance::<TestSpec>(&user_address, &sov_bank::config_gas_token_id(), Some(1))
        .await?;
    // Spent some gas!
    assert!(gas_balance_height_1 < genesis_gas_balance);
    assert_balance(client, 1000, token_id, user_address, None).await?;

    // transfer 100 tokens. assert sender balance. height 3
    let tx = build_transfer_token_tx(&key, token_id, recipient_address, 100, 1);
    let rollup_height = tx_sender.send_txs(client, &[tx]).await?;
    assert_eq!(2, rollup_height);
    let gas_balance_height_2 = client
        .get_balance::<TestSpec>(&user_address, &sov_bank::config_gas_token_id(), Some(2))
        .await?;
    assert!(gas_balance_height_2 < gas_balance_height_1);
    assert_slot_finality(client, rollup_height, test_case.expected_head_finality()).await;

    if test_case.wait_for_aggregated_proof {
        aggregated_proofs_posted_to_da_subscription.recv().await?;
    }

    assert_balance(client, 900, token_id, user_address, None).await?;
    // transfer 200 tokens. assert sender balance. height 4
    let tx = build_transfer_token_tx(&key, token_id, recipient_address, 200, 2);
    let rollup_height = tx_sender.send_txs(client, &[tx]).await?;
    assert_eq!(3, rollup_height);
    assert_slot_finality(client, rollup_height, test_case.expected_head_finality()).await;

    assert_balance(client, 700, token_id, user_address, None).await?;

    // assert sender balance at height 1.
    assert_balance(client, 1000, token_id, user_address, Some(1)).await?;

    // assert sender balance at height 2.
    assert_balance(client, 900, token_id, user_address, Some(2)).await?;

    // assert sender balance at height 3.
    assert_balance(client, 700, token_id, user_address, Some(3)).await?;

    // 10 transfers of 10,11..20
    let transfer_amounts: Vec<u64> = (10u64..20).collect();
    let txs = build_multiple_transfers(&transfer_amounts, &key, token_id, recipient_address, 3);
    let rollup_height = tx_sender.send_txs(client, &txs).await?;
    assert_eq!(4, rollup_height);
    assert_slot_finality(client, rollup_height, test_case.expected_head_finality()).await;

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
        // Because da blocks produced only on submit to DA layer, we can guarantee those rollup heights:
        assert_eq!(1, pub_data.initial_rollup_height);
        assert_eq!(1, pub_data.final_rollup_height);
        assert_aggregated_proof(1, 1, client).await?;
    }

    if let Some(finalized_rollup_height) = test_case.get_latest_finalized_slot_after(rollup_height)
    {
        assert_slot_finality(client, finalized_rollup_height, FinalityStatus::Finalized).await;
    }

    Ok(())
}
