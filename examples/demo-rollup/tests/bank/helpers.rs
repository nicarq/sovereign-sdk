use anyhow::{anyhow, Context};
use demo_stf::runtime::{Runtime, RuntimeCall};
use sov_bank::event::Event as BankEvent;
use sov_bank::{Coins, TokenId};
use sov_cli::NodeClient;
use sov_mock_zkvm::{MockCodeCommitment, MockZkVerifier};
use sov_modules_api::transaction::Transaction;
use sov_modules_api::{
    Address, AggregatedProofPublicData, CryptoSpec, PrivateKey, SafeVec, Spec, Storage,
};
use sov_rollup_interface::node::ledger_api::FinalityStatus;
use sov_rollup_interface::zk::aggregated_proof::AggregateProofVerifier;
use sov_test_utils::default_test_signed_transaction;
use sov_test_utils::test_rollup::read_private_key;

use super::TOKEN_NAME;
use crate::test_helpers::{DemoRollupSpec, CHAIN_HASH};

type TestSpec = DemoRollupSpec;

pub type SequencerTxSender =
    sov_test_utils::tx_sender::SequencerTxSender<Runtime<TestSpec>, TestSpec>;

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

    pub(crate) fn get_latest_finalized_slot_after(&self, rollup_height: u64) -> Option<u64> {
        rollup_height.checked_sub(self.finalization_blocks as u64)
    }
}

pub(crate) fn create_keys_and_addresses() -> (
    <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::PrivateKey,
    <TestSpec as Spec>::Address,
    TokenId,
    <TestSpec as Spec>::Address,
) {
    let key_and_address = read_private_key::<TestSpec>("tx_signer_private_key.json");
    let key = key_and_address.private_key;
    let user_address: <TestSpec as Spec>::Address = key_and_address.address;

    let token_id = sov_bank::get_token_id::<TestSpec>(TOKEN_NAME, &user_address);

    let recipient_key = <<TestSpec as Spec>::CryptoSpec as CryptoSpec>::PrivateKey::generate();
    let address = recipient_key
        .pub_key()
        .to_address::<Address<sha2::Sha256>>();
    let recipient_address = <TestSpec as Spec>::Address::from(address);

    (key, user_address, token_id, recipient_address)
}

pub(crate) fn build_create_token_tx(
    key: &<<TestSpec as Spec>::CryptoSpec as CryptoSpec>::PrivateKey,
    nonce: u64,
    initial_balance: u64,
) -> Transaction<Runtime<TestSpec>, TestSpec> {
    let user_address = key.pub_key().to_address::<Address<sha2::Sha256>>();
    let msg = RuntimeCall::<TestSpec>::Bank(sov_bank::CallMessage::<TestSpec>::CreateToken {
        token_name: TOKEN_NAME.try_into().unwrap(),
        initial_balance,
        mint_to_address: user_address.into(),
        admins: SafeVec::new(),
    });
    default_test_signed_transaction(key, &msg, nonce, &CHAIN_HASH)
}

pub(crate) fn build_transfer_token_tx(
    key: &<<TestSpec as Spec>::CryptoSpec as CryptoSpec>::PrivateKey,
    token_id: TokenId,
    recipient: <TestSpec as Spec>::Address,
    amount: u64,
    nonce: u64,
) -> Transaction<Runtime<TestSpec>, TestSpec> {
    let msg = RuntimeCall::<TestSpec>::Bank(sov_bank::CallMessage::<TestSpec>::Transfer {
        to: recipient,
        coins: Coins { amount, token_id },
    });
    default_test_signed_transaction(key, &msg, nonce, &CHAIN_HASH)
}

pub(crate) fn build_multiple_transfers(
    amounts: &[u64],
    signer_key: &<<TestSpec as Spec>::CryptoSpec as CryptoSpec>::PrivateKey,
    token_id: TokenId,
    recipient: <TestSpec as Spec>::Address,
    start_nonce: u64,
) -> Vec<Transaction<Runtime<TestSpec>, TestSpec>> {
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
    client: &NodeClient,
    assert_amount: u64,
    token_id: TokenId,
    user_address: <TestSpec as Spec>::Address,
    rollup_height: Option<u64>,
) -> anyhow::Result<()> {
    let actual_amount = client
        .get_balance::<TestSpec>(&user_address, &token_id, rollup_height)
        .await
        .with_context(|| {
            format!(
                "Failed to assert balance at rollup_height {:?} for user {} and token {}",
                rollup_height, user_address, token_id
            )
        })?;
    if assert_amount != actual_amount {
        anyhow::bail!(
            "Unexpected amount at rollup_height {:?}. expected={} actual={}",
            rollup_height,
            assert_amount,
            actual_amount,
        )
    }
    Ok(())
}

pub(crate) async fn assert_aggregated_proof(
    initial_slot: u64,
    final_slot: u64,
    client: &NodeClient,
) -> anyhow::Result<()> {
    let proof_response = client.client.get_latest_aggregated_proof().await?;

    let verifier = AggregateProofVerifier::<MockZkVerifier>::new(MockCodeCommitment::default());
    let proof_pub_data: AggregatedProofPublicData<
        <TestSpec as Spec>::Address,
        <TestSpec as Spec>::Da,
        <<TestSpec as Spec>::Storage as Storage>::Root,
    > = verifier.verify(
        &proof_response
            .data
            .clone()
            .ok_or_else(|| anyhow!("data should be defined"))?
            .try_into()?,
    )?;

    // We test inequality because proofs are saved asynchronously in the db.
    assert!(initial_slot <= proof_pub_data.initial_rollup_height.get());
    assert!(final_slot <= proof_pub_data.final_rollup_height.get());

    Ok(())
}

pub(crate) async fn assert_slot_finality(
    client: &NodeClient,
    rollup_height: u64,
    expected_finality: FinalityStatus,
) {
    let slot = client
        .client
        .get_slot_by_id(
            &sov_api_spec::types::IntOrHash::Integer(rollup_height),
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        expected_finality,
        slot.data.as_ref().unwrap().finality_status.into(),
        "Wrong finality status for rollup height {rollup_height}"
    );
}

pub(crate) async fn assert_bank_event<S: Spec>(
    client: &NodeClient,
    event_number: u64,
    expected_event: BankEvent<S>,
) -> anyhow::Result<()> {
    let event_response = client.client.get_event_by_id(event_number).await?;

    // Ensure "Bank" is present in response json
    assert_eq!(event_response.data.as_ref().unwrap().module.name, "Bank");

    let event_value =
        serde_json::Value::Object(event_response.data.as_ref().unwrap().value.clone());

    // Attempt to deserialize the "body" of the bank key in the response to the Event type
    let bank_event_contents = serde_json::from_value::<BankEvent<S>>(event_value)?;

    // Ensure the event generated is a TokenCreated event with the correct token_id
    assert_eq!(bank_event_contents, expected_event);

    Ok(())
}
