use std::sync::Arc;

use demo_stf::genesis_config::GenesisPaths;
use demo_stf::runtime::RuntimeCall;
use sov_bank::event::Event as BankEvent;
use sov_bank::{Coins, TokenId};
use sov_kernels::basic::BasicKernelGenesisPaths;
use sov_mock_da::storable::service::StorableMockDaService;
use sov_mock_da::{BlockProducingConfig, MockAddress, MockDaConfig, MockDaSpec};
use sov_mock_zkvm::{MockCodeCommitment, MockZkVerifier};
use sov_modules_api::rest::utils::{ErrorObject, ResponseObject};
use sov_modules_api::transaction::{Transaction, UnsignedTransaction};
use sov_modules_api::{PrivateKey, Spec};
use sov_modules_macros::config_value;
use sov_rollup_interface::node::da::DaServiceWithRetries;
use sov_rollup_interface::node::ledger_api::FinalityStatus;
use sov_rollup_interface::zk::aggregated_proof::AggregateProofVerifier;
use sov_stf_runner::RollupProverConfig;
use sov_test_utils::{
    ApiClient, TestPrivateKey, TestSpec, TEST_DEFAULT_MAX_FEE, TEST_DEFAULT_MAX_PRIORITY_FEE,
};
use tokio::task::JoinHandle;

use super::{TOKEN_NAME, TOKEN_SALT};
use crate::test_helpers::{read_private_keys, start_rollup_in_background};

pub(crate) struct TestCase {
    pub(crate) wait_for_aggregated_proof: bool,
    pub(crate) finalization_blocks: u32,
}

impl TestCase {
    pub(crate) fn expected_head_finality(&self) -> FinalityStatus {
        match self.finalization_blocks {
            0 => FinalityStatus::Finalized,
            _ => FinalityStatus::Pending,
        }
    }

    pub(crate) fn get_latest_finalized_slot_after(&self, slot_number: u64) -> Option<u64> {
        slot_number.checked_sub(self.finalization_blocks as u64)
    }
}

pub(crate) struct TestRollup {
    pub(crate) rollup_task: JoinHandle<()>,
    pub(crate) client: ApiClient,
    pub(crate) da_service: Arc<DaServiceWithRetries<StorableMockDaService>>,
}

impl TestRollup {
    pub(crate) async fn create_test_rollup(
        test_case: &TestCase,
        rollup_prover_config: RollupProverConfig,
        block_producing: BlockProducingConfig,
    ) -> anyhow::Result<TestRollup> {
        let (rpc_port_tx, rpc_port_rx) = tokio::sync::oneshot::channel();
        let (rest_port_tx, rest_port_rx) = tokio::sync::oneshot::channel();

        // This value is important and should match ../test-data/genesis/integration-tests /sequencer_registry.json
        // Otherwise batches are going to be rejected
        let sequencer_address = MockAddress::new([0; 32]);
        let block_time_ms = 10_000;
        let storable_mock_da_connection_string = "sqlite::memory:".to_string();

        let mock_da_config = MockDaConfig {
            connection_string: storable_mock_da_connection_string,
            sender_address: sequencer_address,
            finalization_blocks: test_case.finalization_blocks,
            block_producing,
            block_time_ms,
        };

        let (rollup_task, da_service) = start_rollup_in_background(
            rpc_port_tx,
            rest_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            rollup_prover_config,
            mock_da_config,
        )
        .await;

        let rpc_port = rpc_port_rx.await.unwrap().port();
        let rest_port = rest_port_rx.await.unwrap().port();
        let client = ApiClient::new(rpc_port, rest_port).await?;

        Ok(TestRollup {
            rollup_task,
            client,
            da_service,
        })
    }
}

pub(crate) fn create_keys_and_addresses() -> (
    TestPrivateKey,
    <TestSpec as Spec>::Address,
    TokenId,
    <TestSpec as Spec>::Address,
) {
    let key_and_address = read_private_keys::<TestSpec>("tx_signer_private_key.json");
    let key = key_and_address.private_key;
    let user_address: <TestSpec as Spec>::Address = key_and_address.address;

    let token_id = sov_bank::get_token_id::<TestSpec>(TOKEN_NAME, &user_address, TOKEN_SALT);

    let recipient_key = TestPrivateKey::generate();
    let recipient_address: <TestSpec as Spec>::Address = recipient_key.to_address();

    (key, user_address, token_id, recipient_address)
}

pub(crate) fn build_create_token_tx(key: &TestPrivateKey, nonce: u64) -> Transaction<TestSpec> {
    let user_address: <TestSpec as Spec>::Address = key.to_address();
    let msg =
        RuntimeCall::<TestSpec, MockDaSpec>::Bank(sov_bank::CallMessage::<TestSpec>::CreateToken {
            salt: TOKEN_SALT,
            token_name: TOKEN_NAME.to_string(),
            initial_balance: 1000,
            mint_to_address: user_address,
            authorized_minters: vec![],
        });
    let chain_id = config_value!("CHAIN_ID");
    let max_priority_fee_bips = TEST_DEFAULT_MAX_PRIORITY_FEE;
    let max_fee = TEST_DEFAULT_MAX_FEE;
    let gas_limit = None;
    Transaction::<TestSpec>::new_signed_tx(
        key,
        UnsignedTransaction::new(
            borsh::to_vec(&msg).unwrap(),
            chain_id,
            max_priority_fee_bips,
            max_fee,
            nonce,
            gas_limit,
        ),
    )
}

pub(crate) fn build_transfer_token_tx(
    key: &TestPrivateKey,
    token_id: TokenId,
    recipient: <TestSpec as Spec>::Address,
    amount: u64,
    nonce: u64,
) -> Transaction<TestSpec> {
    let msg =
        RuntimeCall::<TestSpec, MockDaSpec>::Bank(sov_bank::CallMessage::<TestSpec>::Transfer {
            to: recipient,
            coins: Coins { amount, token_id },
        });
    let chain_id = config_value!("CHAIN_ID");
    let max_priority_fee_bips = TEST_DEFAULT_MAX_PRIORITY_FEE;
    let max_fee = TEST_DEFAULT_MAX_FEE;
    let gas_limit = None;
    Transaction::<TestSpec>::new_signed_tx(
        key,
        UnsignedTransaction::new(
            borsh::to_vec(&msg).unwrap(),
            chain_id,
            max_priority_fee_bips,
            max_fee,
            nonce,
            gas_limit,
        ),
    )
}

pub(crate) fn build_multiple_transfers(
    amounts: &[u64],
    signer_key: &TestPrivateKey,
    token_id: TokenId,
    recipient: <TestSpec as Spec>::Address,
    start_nonce: u64,
) -> Vec<Transaction<TestSpec>> {
    let mut txs = vec![];
    let mut nonce = start_nonce;
    for amt in amounts {
        txs.push(build_transfer_token_tx(
            signer_key, token_id, recipient, *amt, nonce,
        ));
        nonce += 1;
    }
    txs
}

pub(crate) async fn assert_balance(
    client: &ApiClient,
    assert_amount: u64,
    token_id: TokenId,
    user_address: <TestSpec as Spec>::Address,
    version: Option<u64>,
) -> anyhow::Result<()> {
    let balance_response_rpc = sov_bank::BankRpcClient::<TestSpec>::balance_of(
        &client.rpc,
        version,
        user_address,
        token_id,
    )
    .await?;
    assert_eq!(
        balance_response_rpc.amount.unwrap_or_default(),
        assert_amount
    );

    let height_param: String = version
        .map(|h| format!("?rollup_height={}", h))
        .unwrap_or_default();
    let balance_url = format!(
        "/modules/bank/tokens/{}/balances/{}{}",
        token_id, user_address, height_param
    );

    let balance_response_rest = client
        .query_rest_endpoint::<ResponseObject<Coins>>(&balance_url)
        .await?;

    assert_eq!(Vec::<ErrorObject>::new(), balance_response_rest.errors);
    let rest_amount = balance_response_rest.data.unwrap().amount;

    assert_eq!(assert_amount, rest_amount);

    Ok(())
}

pub(crate) async fn assert_aggregated_proof(
    initial_slot: u64,
    final_slot: u64,
    client: &ApiClient,
) -> anyhow::Result<()> {
    let proof_response = client.ledger.get_latest_aggregated_proof().await?;

    let verifier = AggregateProofVerifier::<MockZkVerifier>::new(MockCodeCommitment::default());
    verifier.verify(&proof_response.data.clone().try_into()?)?;

    let proof_pub_data = &proof_response.data.public_data;
    // We test inequality because proofs are saved asynchronously in the db.
    assert!(initial_slot <= proof_pub_data.initial_slot_number);
    assert!(final_slot <= proof_pub_data.final_slot_number);

    let proof_data_info_response = client.ledger.get_latest_aggregated_proof().await?;

    assert!(
        initial_slot
            <= proof_data_info_response
                .data
                .public_data
                .initial_slot_number
    );
    assert!(final_slot <= proof_data_info_response.data.public_data.final_slot_number);

    Ok(())
}

pub(crate) fn assert_aggregated_proof_public_data(
    expected_initial_slot_number: u64,
    expected_final_slot_number: u64,
    pub_data: &sov_ledger_json_client::types::AggregatedProofPublicData,
) {
    assert_eq!(expected_initial_slot_number, pub_data.initial_slot_number);
    assert_eq!(expected_final_slot_number, pub_data.final_slot_number);
}

pub(crate) async fn assert_slot_finality(
    client: &ApiClient,
    slot_number: u64,
    expected_finality: FinalityStatus,
) {
    let slot = client
        .ledger
        .get_slot_by_id(
            &sov_ledger_json_client::types::IntOrHash::Variant0(slot_number),
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        expected_finality,
        slot.data.finality_status.into(),
        "Wrong finality status for slot number {slot_number}"
    );
}

pub(crate) async fn assert_bank_event<S: Spec>(
    client: &ApiClient,
    event_number: u64,
    expected_event: BankEvent<S>,
) -> anyhow::Result<()> {
    let event_response = client.ledger.get_event_by_id(event_number).await?;

    // Ensure "Bank" is present in response json
    assert_eq!(event_response.data.module.name, "Bank");

    let event_value = serde_json::Value::Object(event_response.data.value.clone());

    // Attempt to deserialize the "body" of the bank key in the response to the Event type
    let bank_event_contents = serde_json::from_value::<BankEvent<S>>(event_value)?;

    // Ensure the event generated is a TokenCreated event with the correct token_id
    assert_eq!(bank_event_contents, expected_event);

    Ok(())
}
