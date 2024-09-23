use demo_stf::genesis_config::GenesisPaths;
use serde::Deserialize;
use sov_kernels::basic::BasicKernelGenesisPaths;
use sov_mock_da::BlockProducingConfig;
use sov_modules_api::rest::utils::ResponseObject;
use sov_test_utils::{ApiClient, TestSpec};

use crate::bank::helpers::*;
use crate::bank::{SequencerTxSender, TxSender, TOKEN_NAME, TOKEN_SALT};
use crate::test_helpers::*;

const BLOCK_PRODUCING_CONFIG: BlockProducingConfig = BlockProducingConfig::OnSubmit;

#[tokio::test(flavor = "multi_thread")]
async fn bank_tx_tests() -> anyhow::Result<()> {
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

    assert_eq!(token_id, token_id_response);

    // create token. height 2
    let initial_balance = 1000;
    let tx = build_create_token_tx(&key, 0, initial_balance);
    let slot_number = tx_sender.send_txs(client, &[tx]).await?;
    assert_eq!(1, slot_number);
    assert_slot_finality(client, slot_number, test_case.expected_head_finality()).await;

    assert_balance(client, initial_balance, token_id, user_address, None).await?;

    // Make 10 transfers
    for i in 1..11 {
        let tx = build_transfer_token_tx(&key, token_id, recipient_address, 10, i);
        let slot_number = tx_sender.send_txs(client, &[tx]).await?;
        assert_eq!(i + 1, slot_number);
        assert_slot_finality(client, slot_number, test_case.expected_head_finality()).await;
        assert_balance(
            client,
            initial_balance - i * 10,
            token_id,
            user_address,
            None,
        )
        .await?;

        // Check max_attested_height from the previous slot.
        let max_attested_height = get_max_attested_height(client).await?;
        assert_eq!(i, max_attested_height);
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct ValueResponse {
    value: u64,
}

async fn get_max_attested_height(client: &ApiClient) -> anyhow::Result<u64> {
    let response = client
        .query_rest_endpoint::<ResponseObject<ValueResponse>>(
            "/modules/attester-incentives/state/maximum-attested-height",
        )
        .await?;

    let height = response.data.unwrap().value;
    Ok(height)
}
