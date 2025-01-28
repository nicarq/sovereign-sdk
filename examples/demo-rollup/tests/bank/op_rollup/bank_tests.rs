use serde::Deserialize;
use sov_cli::NodeClient;
use sov_demo_rollup::{mock_da_risc0_host_args, MockDemoRollup};
use sov_mock_da::BlockProducingConfig;
use sov_modules_api::execution_mode::Native;
use sov_modules_api::rest::utils::ResponseObject;
use sov_modules_api::OperatingMode;
use sov_test_utils::test_rollup::RollupBuilder;
use sov_test_utils::tx_sender::TxSender;

use crate::bank::helpers::*;
use crate::bank::TOKEN_NAME;
use crate::test_helpers::*;

const BLOCK_PRODUCING_CONFIG: BlockProducingConfig = BlockProducingConfig::Periodic;

#[tokio::test(flavor = "multi_thread")]
async fn flaky_bank_tx_tests() -> anyhow::Result<()> {
    let test_case = TestCase {
        wait_for_aggregated_proof: true,
        finalization_blocks: 0,
    };

    let test_rollup = RollupBuilder::<MockDemoRollup<Native>>::new(
        test_genesis_source(OperatingMode::Optimistic),
        BLOCK_PRODUCING_CONFIG,
        test_case.finalization_blocks,
    )
    .with_zkvm_host_args(mock_da_risc0_host_args())
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
    let (key, user_address, token_id, recipient_address) = create_keys_and_addresses();
    let token_id_response = client
        .get_token_id::<DemoRollupSpec>(TOKEN_NAME, &user_address)
        .await?;

    const NUM_TRANSFERS: u64 = 5;

    assert_eq!(token_id, token_id_response);

    // create token.
    let initial_balance = 1000;
    let tx = build_create_token_tx(&key, 0, initial_balance);
    let slot_number = tx_sender.send_txs(client, &[tx]).await?;

    assert_slot_finality(client, slot_number, test_case.expected_head_finality()).await;
    assert_balance(client, initial_balance, token_id, user_address, None).await?;

    // Make a few transfers and check that attestation height progresses
    for nonce in 1..=NUM_TRANSFERS {
        let tx = build_transfer_token_tx(&key, token_id, recipient_address, 10, nonce);
        let slot_number = tx_sender.send_txs(client, &[tx]).await?;
        assert_slot_finality(client, slot_number, test_case.expected_head_finality()).await;
        assert_balance(
            client,
            initial_balance - nonce * 10,
            token_id,
            user_address,
            None,
        )
        .await?;

        let mut max_attested_height = get_max_attested_height(client).await?;
        while max_attested_height < slot_number {
            // In some cases, the attestation may not be ready just yet. Let's try again in a bit.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            max_attested_height = get_max_attested_height(client).await?;
        }
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct ValueResponse {
    value: u64,
}

async fn get_max_attested_height(client: &NodeClient) -> anyhow::Result<u64> {
    let response = client
        .query_rest_endpoint::<ResponseObject<ValueResponse>>(
            "/modules/attester-incentives/state/maximum-attested-height",
        )
        .await?;

    let height = response.data.unwrap().value;
    Ok(height)
}
