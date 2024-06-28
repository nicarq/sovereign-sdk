use demo_stf::runtime::Runtime;
use sov_bank::Bank;
use sov_celestia_adapter::verifier::CelestiaSpec;
use sov_celestia_adapter::CelestiaService;
use sov_modules_api::capabilities::Authenticator;
use sov_modules_api::transaction::{PriorityFeeBips, Transaction, UnsignedTransaction};
use sov_modules_api::{BlobData, EncodeCall, RawTx};
use sov_rollup_interface::services::da::DaService;

use crate::types::{Auth, PreparedCallMessage, ThisSpec};
use crate::{AccountPool, Args};

pub async fn submit_transactions(
    da_service: &CelestiaService,
    txs: Vec<RawTx>,
) -> anyhow::Result<()> {
    let batch = BlobData::new_batch(txs);
    let batch_bytes = borsh::to_vec(&batch).expect("Failed to serialize batch");
    let fee = da_service.estimate_fee(batch_bytes.len()).await.unwrap();
    let tx_hash = da_service.send_transaction(&batch_bytes, fee).await?;
    tracing::info!(%tx_hash, "Submitted Tx");
    Ok(())
}

pub fn prepare_message_for_submission(
    config: &Args,
    account_pool: &mut AccountPool<ThisSpec>,
    prepared_message: PreparedCallMessage<ThisSpec, Bank<ThisSpec>>,
) -> RawTx {
    let PreparedCallMessage {
        call_message,
        from,
        max_fee,
    } = prepared_message;
    tracing::debug!(?call_message, %from, "Iterating over call message");
    let account = account_pool.get_mut_account(&from).unwrap();
    let nonce = account.nonce;
    let runtime_msg =
        <Runtime<ThisSpec, CelestiaSpec> as EncodeCall<Bank<ThisSpec>>>::encode_call(call_message);
    let unsigned_tx = UnsignedTransaction {
        runtime_msg,
        chain_id: config.chain_id,
        max_priority_fee_bips: PriorityFeeBips::from_percentage(config.priority_fee_percent),
        max_fee,
        nonce,
        gas_limit: None,
    };
    let tx = Transaction::<ThisSpec>::new_signed_tx(&account.private_key, unsigned_tx);
    let authed_tx = Auth::encode(borsh::to_vec(&tx).unwrap()).unwrap();
    account.nonce += 1;
    authed_tx
}
