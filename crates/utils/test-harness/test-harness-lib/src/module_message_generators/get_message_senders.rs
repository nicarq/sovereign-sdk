use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use sov_bank::{Bank, GAS_TOKEN_ID};
use sov_modules_api::prelude::tokio;
use sov_modules_api::Spec;
use sov_prover_incentives::ProverIncentives;
use sov_rollup_interface::services::da::DaService;
use tokio::sync::mpsc::Sender;

use super::bank::{TokenCreationMessageGenerator, TokenTransferMessageGenerator};
use super::prover_incentives::ProverIncentivesMessageGenerator;
use super::{MessageSender, MessageSenderT};
use crate::{AccountPool, SerializedPreparedCallMessage};

/// This function collates a list of `MessageSender`s to be consumed by whomever is the receiver
/// of said messages. When implementing a new message sender for a given module, add it here.
pub fn get_message_senders<S: Spec, Da: DaService>(
    should_stop: Arc<AtomicBool>,
    account_pool: AccountPool<S>,
    serialized_messages_tx: Sender<SerializedPreparedCallMessage>,
) -> anyhow::Result<Vec<Box<dyn MessageSenderT>>> {
    Ok(vec![
        Box::new(MessageSender::<
            demo_stf::runtime::Runtime<S, <Da as DaService>::Spec>,
            <Da as DaService>::Spec,
            S,
            Bank<S>,
        >::new(
            "token creator",
            should_stop.clone(),
            Box::new(TokenCreationMessageGenerator::new_from_account_pool(
                account_pool.clone(),
            )),
            serialized_messages_tx.clone(),
        )),
        Box::new(MessageSender::<
            demo_stf::runtime::Runtime<S, <Da as DaService>::Spec>,
            <Da as DaService>::Spec,
            S,
            Bank<S>,
        >::new(
            "token sender",
            should_stop.clone(),
            Box::new(TokenTransferMessageGenerator::new_from_account_pool(
                account_pool.clone(),
                GAS_TOKEN_ID,
            )?),
            serialized_messages_tx.clone(),
        )),
        Box::new(MessageSender::<
            demo_stf::runtime::Runtime<S, <Da as DaService>::Spec>,
            <Da as DaService>::Spec,
            S,
            ProverIncentives<S, <Da as DaService>::Spec>,
        >::new(
            "prover incentives",
            should_stop.clone(),
            Box::new(ProverIncentivesMessageGenerator::new_from_account_pool(
                account_pool.clone(),
            )),
            serialized_messages_tx.clone(),
        )),
    ])
}
