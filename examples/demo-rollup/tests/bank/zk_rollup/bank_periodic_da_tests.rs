use anyhow::Context;
use futures::StreamExt;
use sov_api_spec::types::AggregatedProof as ApiAggregatedProof;
use sov_cli::NodeClient;
use sov_demo_rollup::{mock_da_risc0_host_args, MockDemoRollup};
use sov_mock_da::BlockProducingConfig;
use sov_mock_zkvm::{MockCodeCommitment, MockZkVerifier};
use sov_modules_api::execution_mode::Native;
use sov_modules_api::{OperatingMode, SerializedAggregatedProof, Spec};
use sov_rollup_interface::zk::aggregated_proof::{
    AggregateProofVerifier, AggregatedProofPublicData,
};
use sov_sequencer::batch_builders::preferred::PreferredBatchBuilderConfig;
use sov_sequencer::BatchBuilderConfig;
use sov_state::Storage;
use sov_stf_runner::processes::RollupProverConfig;
use sov_test_utils::test_rollup::RollupBuilder;
use sov_test_utils::tx_sender::TxSender;

use crate::bank::helpers::*;
use crate::bank::TOKEN_NAME;
use crate::test_helpers::{test_genesis_source, DemoRollupSpec};

type TestSpec = DemoRollupSpec;

#[tokio::test(flavor = "multi_thread")]
async fn flaky_bank_tx_tests_periodic_da() -> anyhow::Result<()> {
    let test_case = TestCase {
        wait_for_aggregated_proof: true,
        finalization_blocks: 0,
    };

    let test_rollup = RollupBuilder::<MockDemoRollup<Native>>::new(
        test_genesis_source(OperatingMode::Zk),
        BlockProducingConfig::Periodic,
        test_case.finalization_blocks,
        0,
        mock_da_risc0_host_args(),
    )
    .set_config(|c| {
        c.rollup_prover_config = RollupProverConfig::Skip;
        c.batch_builder_config = BatchBuilderConfig::Preferred(PreferredBatchBuilderConfig {
            // FIXME(@theochap): It seems this test is broken because the sequencer state does
            // not update fast enough. Hence we disable the state update here.
            should_update_state: false,
            ..Default::default()
        });
    })
    .start()
    .await?;

    let sender = SequencerTxSender::default();

    // If the rollup throws an error, return it and stop trying to send the transaction
    tokio::select! {
        err = test_rollup.rollup_task => err?,
        res = send_test_bank_txs(test_case, &test_rollup.client, sender) => Ok(res?),
    }?;

    Ok(())
}

async fn send_test_bank_txs(
    test_case: TestCase,
    client: &NodeClient,
    tx_sender: SequencerTxSender,
) -> anyhow::Result<()> {
    // There's no guarantee that we subscribed before the first proof is published.
    // But we know that it should be less or equal rollup_height of the first published batch
    let mut aggregated_proof_subscription = client
        .client
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
        let aggregated_proof_resp: ApiAggregatedProof =
            aggregated_proof_subscription.next().await.unwrap().unwrap();

        let proof: SerializedAggregatedProof = aggregated_proof_resp.try_into()?;
        let verifier = AggregateProofVerifier::<MockZkVerifier>::new(MockCodeCommitment::default());
        let pub_data: AggregatedProofPublicData<
            <TestSpec as Spec>::Address,
            <TestSpec as Spec>::Da,
            <<TestSpec as Spec>::Storage as Storage>::Root,
        > = verifier.verify(&proof)?;

        assert!(slot_batch_1 >= pub_data.initial_rollup_height);
        assert!(slot_batch_1 >= pub_data.final_rollup_height);
        // We can only check this under periodic block producing.
        // More thorough checks should be done in "OnSubmit" batch producing
        assert_aggregated_proof(1, 1, client).await?;
    }
    Ok(())
}
