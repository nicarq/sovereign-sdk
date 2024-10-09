use anyhow::Context;
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

    // create token. height 1
    let initial_balance = 1000;
    let tx = build_create_token_tx(&key, 0, initial_balance);
    let batch_1_slot_number = tx_sender.send_txs(client, &[tx]).await?;

    assert!(batch_1_slot_number >= 1);

    assert_slot_finality(
        client,
        batch_1_slot_number,
        test_case.expected_head_finality(),
    )
    .await;

    assert_balance(client, initial_balance, token_id, user_address, None)
        .await
        .context("Initial balance after token create")?;

    // Since we don't have control on which DA blocks attestation lends,
    // We check that all slots up to slot with transactions has been attested.

    let mut rollup_height = 1;
    let mut verified_attested_height = 0;
    // How many slots rollup allowed to lag behind in posting attestations
    let attestation_publish_threshold = 1000;

    while verified_attested_height <= batch_1_slot_number {
        let slot = slots_subscription.next().await.unwrap()?;
        assert!(slot.number >= rollup_height);
        let max_attested_height = get_max_attested_height(client, Some(rollup_height)).await?;
        if max_attested_height == verified_attested_height {
            verified_attested_height += 1;
        }
        rollup_height += 1;
        if rollup_height > (batch_1_slot_number + attestation_publish_threshold) {
            panic!(
                "Attestations haven't been posted after {} slots passed since batch publication",
                attestation_publish_threshold
            );
        }
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct ValueResponse {
    value: u64,
}

async fn get_max_attested_height(
    client: &NodeClient,
    rollup_height: Option<u64>,
) -> anyhow::Result<u64> {
    let param = rollup_height
        .map(|h| format!("?rollup_height={}", h))
        .unwrap_or_default();
    let url = format!(
        "/modules/attester-incentives/state/maximum-attested-height{}",
        param
    );
    let response = client
        .query_rest_endpoint::<ResponseObject<ValueResponse>>(&url)
        .await?;

    let height = response.data.unwrap().value;
    Ok(height)
}
