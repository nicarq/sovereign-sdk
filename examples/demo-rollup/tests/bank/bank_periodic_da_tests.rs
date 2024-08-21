use anyhow::Context;
use futures::StreamExt;
use sov_mock_da::BlockProducingConfig;
use sov_test_utils::{ApiClient, TestSpec};

use super::helpers::*;
use super::TxSender;
use crate::bank::{SequencerTxSender, TOKEN_NAME, TOKEN_SALT};
use crate::test_helpers::get_appropriate_rollup_prover_config;

#[tokio::test(flavor = "multi_thread")]
async fn bank_tx_tests_periodic_da() -> anyhow::Result<()> {
    let test_case = TestCase {
        wait_for_aggregated_proof: true,
        finalization_blocks: 0,
    };

    let test_rollup = TestRollup::create_test_rollup(
        &test_case,
        get_appropriate_rollup_prover_config(),
        BlockProducingConfig::Periodic,
    )
    .await?;

    let sender = SequencerTxSender {};

    // If the rollup throws an error, return it and stop trying to send the transaction
    tokio::select! {
        err = test_rollup.rollup_task => err?,
        res = send_test_bank_txs(test_case, &test_rollup.client, sender) => res?,
    };

    Ok(())
}

async fn send_test_bank_txs(
    test_case: TestCase,
    client: &ApiClient,
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

    let mut aggregated_proof_subscription = client
        .ledger
        .subscribe_aggregated_proof()
        .await
        .context("Failed to subscribe to aggregated proof")?;

    assert_eq!(token_id, token_id_response);

    let tx = build_create_token_tx(&key, 0);
    tx_sender.send_txs(client, &[tx]).await?;
    assert_balance(client, 1000, token_id, user_address, None).await?;

    // transfer 100 tokens. assert sender balance.
    let tx = build_transfer_token_tx(&key, token_id, recipient_address, 100, 1);
    tx_sender.send_txs(client, &[tx]).await?;
    assert_balance(client, 900, token_id, user_address, None).await?;

    // transfer 200 tokens. assert sender balance.
    let tx = build_transfer_token_tx(&key, token_id, recipient_address, 200, 2);
    tx_sender.send_txs(client, &[tx]).await?;
    assert_balance(client, 700, token_id, user_address, None).await?;

    if test_case.wait_for_aggregated_proof {
        let aggregated_proof_resp = aggregated_proof_subscription.next().await.unwrap()?;
        let pub_data = aggregated_proof_resp.public_data;
        assert_aggregated_proof_public_data(1, 1, &pub_data);
        assert_aggregated_proof(1, 1, client).await?;
    }
    Ok(())
}
