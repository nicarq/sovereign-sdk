use anyhow::Context;
use demo_stf::genesis_config::GenesisPaths;
use futures::StreamExt;
use sov_cli::NodeClient;
use sov_kernels::basic::BasicKernelGenesisPaths;
use sov_mock_da::BlockProducingConfig;
use sov_test_utils::TestSpec;

use crate::bank::helpers::*;
use crate::bank::{SequencerTxSender, TxSender, TOKEN_NAME};
use crate::test_helpers::*;

#[tokio::test(flavor = "multi_thread")]
async fn bank_tx_tests_periodic_da() -> anyhow::Result<()> {
    let test_case = TestCase {
        wait_for_aggregated_proof: true,
        finalization_blocks: 0,
    };

    let test_rollup = TestRollup::create_test_rollup(
        get_appropriate_rollup_prover_config(),
        BlockProducingConfig::Periodic,
        test_case.finalization_blocks,
        GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
        BasicKernelGenesisPaths {
            chain_state: "../test-data/genesis/integration-tests/chain_state_zk.json".into(),
        },
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
    client: &NodeClient,
    tx_sender: impl TxSender,
) -> anyhow::Result<()> {
    // There's no guarantee that we subscribed before the first proof is published.
    // But we know that it should be less or equal rollup_height of the first published batch
    let mut aggregated_proof_subscription = client
        .ledger
        .subscribe_aggregated_proof()
        .await
        .context("Failed to subscribe to aggregated proof")?;

    let (key, user_address, token_id, recipient_address) = create_keys_and_addresses();
    let token_id_response = client
        .get_token_id::<TestSpec>(TOKEN_NAME, &user_address)
        .await?;

    assert_eq!(token_id, token_id_response);

    let tx = build_create_token_tx(&key, 0, 1000);
    let slot_batch_1 = tx_sender.send_txs(client, &[tx]).await?;

    assert_balance(client, 1000, token_id, user_address, None)
        .await
        .context("Initial balance at latest version")?;

    // transfer 100 tokens. assert sender balance.
    let tx = build_transfer_token_tx(&key, token_id, recipient_address, 100, 1);
    let _slot_batch_2 = tx_sender.send_txs(client, &[tx]).await?;
    assert_balance(client, 900, token_id, user_address, None)
        .await
        .context("Balance decreased after first transaction, latest version")?;

    // transfer 200 tokens. assert sender balance.
    let tx = build_transfer_token_tx(&key, token_id, recipient_address, 200, 2);
    let _slot_batch_3 = tx_sender.send_txs(client, &[tx]).await?;
    assert_balance(client, 700, token_id, user_address, None)
        .await
        .context("Balance decreased after second transaction, latest version")?;

    if test_case.wait_for_aggregated_proof {
        let aggregated_proof_resp = aggregated_proof_subscription.next().await.unwrap()?;
        let pub_data = aggregated_proof_resp.public_data;
        assert!(slot_batch_1 >= pub_data.initial_slot_number);
        assert!(slot_batch_1 >= pub_data.final_slot_number);
        // We can only check this under periodic block producing.
        // More thorough checks should be done in "OnSubmit" batch producing
        assert_aggregated_proof(1, 1, client).await?;
    }
    Ok(())
}
