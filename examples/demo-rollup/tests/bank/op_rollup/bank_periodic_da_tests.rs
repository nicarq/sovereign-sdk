use demo_stf::genesis_config::GenesisPaths;
use futures::StreamExt;
use serde::Deserialize;
use sov_cli::NodeClient;
use sov_kernels::basic::BasicKernelGenesisPaths;
use sov_mock_da::BlockProducingConfig;
use sov_modules_api::rest::utils::ResponseObject;
use sov_test_utils::TestSpec;

use crate::bank::helpers::*;
use crate::bank::{SequencerTxSender, TxSender, TOKEN_NAME};
use crate::test_helpers::*;

const BLOCK_PRODUCING_CONFIG: BlockProducingConfig = BlockProducingConfig::Periodic;

#[tokio::test(flavor = "multi_thread")]
async fn bank_tx_periodic_da_tests() -> anyhow::Result<()> {
    let test_case = TestCase {
        wait_for_aggregated_proof: true,
        finalization_blocks: 0,
    };

    let test_rollup = TestRollup::create_test_rollup(
        get_appropriate_rollup_prover_config(),
        BLOCK_PRODUCING_CONFIG,
        test_case.finalization_blocks,
        GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
        BasicKernelGenesisPaths {
            chain_state: "../test-data/genesis/integration-tests/chain_state_op.json".into(),
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
    let mut slots_subscription = client.ledger.subscribe_slots().await?;

    let (key, user_address, token_id, _recipient_address) = create_keys_and_addresses();
    let token_id_response = client
        .get_token_id::<TestSpec>(TOKEN_NAME, &user_address)
        .await?;

    assert_eq!(token_id, token_id_response);

    // create token. height 2
    let initial_balance = 1000;
    let tx = build_create_token_tx(&key, 0, initial_balance);
    let slot_number = tx_sender.send_txs(client, &[tx]).await?;
    assert_eq!(1, slot_number);
    assert_slot_finality(client, slot_number, test_case.expected_head_finality()).await;

    assert_balance(client, initial_balance, token_id, user_address, None).await?;

    for _ in 0..3 {
        let slot = slots_subscription.next().await.unwrap()?;
        let max_attested_height = get_max_attested_height(client).await?;
        assert_eq!(slot.number - 1, max_attested_height);
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
