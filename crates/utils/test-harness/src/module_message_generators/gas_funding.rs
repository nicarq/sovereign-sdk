use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use sov_bank::{Amount, Bank, CallMessage, GAS_TOKEN_ID};
use sov_cli::NodeClient;
use sov_modules_api::prelude::tokio;
use sov_modules_api::Spec;
use sov_modules_stf_blueprint::Runtime;
use sov_rollup_interface::da::DaSpec;
use tokio::sync::mpsc::Sender;

use super::{MessageSender, MessageSenderT};
use crate::account_pool::AccountPool;
use crate::constants::DEFAULT_MAX_FEE;
use crate::{PreparedCallMessage, SerializedPreparedCallMessage};

// How much funds account should have to be considered a "whale".
const MINIMAL_WHALE_BALANCE: u64 = 5_000_000;

/// This function creates the call messages required to mint and distribute the rollup's gas-token -
/// as defined in the genesis config - to the set of accounts in the account pool.
pub async fn get_gas_funding_txs<S: Spec>(
    node_url: &str,
    account_pool: &AccountPool<S>,
) -> anyhow::Result<Vec<PreparedCallMessage<S, Bank<S>>>> {
    let node_client = NodeClient::new(node_url).await?;

    let gas_whale_account_pool_indices = {
        let mut map = HashMap::<&S::Address, u64>::new();
        // Skip generated accounts as we know they don't existing on the rollup.
        for address in account_pool.imported_addresses() {
            let balance = node_client.get_balance::<S>(address, &GAS_TOKEN_ID).await?;
            if balance >= MINIMAL_WHALE_BALANCE {
                let index = account_pool.get_index(address).expect("Impossible happened: imported account cannot be mapped to index back by address");
                map.insert(address, *index);
            }
        }
        map
    };

    let num_gas_whales = gas_whale_account_pool_indices.len();

    if num_gas_whales == 0 {
        anyhow::bail!("No whales found!");
    }

    let total_supply = node_client
        .get_total_supply(&GAS_TOKEN_ID)
        .await
        .expect("Gas token should exist");

    let mut txs = Vec::new();

    let enough_supply = Amount::MAX / 2;
    if total_supply < enough_supply {
        let gas_token_minter_account_pool_index = account_pool.gas_token_minter_index();
        let gas_token_minter = account_pool
            .get_address_by_index(&gas_token_minter_account_pool_index)
            .expect("gas token minter address should exist in account pool!");
        tracing::info!(gas_token_minter = %gas_token_minter, account_pool_index = %gas_token_minter_account_pool_index, "Gas token minter");
        tracing::info!(
            total_supply,
            enough = enough_supply,
            "Total supply of gas token is not large enough, need to mint!"
        );
        let to_mint = Amount::MAX - 100 - total_supply;
        let to_mint_per_whale = to_mint / num_gas_whales as u64;
        for &whale_address in gas_whale_account_pool_indices.keys() {
            tracing::info!(amount = to_mint_per_whale, to = %whale_address, from = %gas_token_minter, account_pool_index = gas_token_minter_account_pool_index, "Mint call message");
            let call_message = CallMessage::<S>::Mint {
                coins: sov_bank::Coins {
                    amount: to_mint_per_whale,
                    token_id: GAS_TOKEN_ID,
                },
                mint_to_address: whale_address.clone(),
            };
            txs.push(PreparedCallMessage::<S, Bank<S>>::new(
                call_message,
                gas_token_minter_account_pool_index,
                DEFAULT_MAX_FEE,
            ));
        }
    }
    tracing::info!("Filling gas balance for all non-whale accounts in account pool");

    let accounts_to_fill = account_pool
        .addresses()
        .filter(|addr| !gas_whale_account_pool_indices.contains_key(addr));

    for (idx, account) in accounts_to_fill.enumerate() {
        let whales: Vec<&S::Address> = gas_whale_account_pool_indices.keys().cloned().collect();
        let whale_idx = idx % num_gas_whales;
        let whale = whales[whale_idx];
        let whale_account_pool_index = *gas_whale_account_pool_indices
            .get(whale)
            .expect("gas whale should exist in account pool");

        // TODO: Better calculation on how much gas will be needed for account.
        let amount = 100_000_000;

        tracing::debug!(whale = %whale, whale_index = whale_idx, whale_acount_pool_index = whale_account_pool_index, "whale info");
        let call_message = CallMessage::<S>::Transfer {
            to: account.clone(),
            coins: sov_bank::Coins {
                amount,
                token_id: GAS_TOKEN_ID,
            },
        };

        txs.push(PreparedCallMessage::<S, Bank<S>>::new(
            call_message,
            whale_account_pool_index,
            DEFAULT_MAX_FEE,
        ));
    }

    Ok(txs)
}

/// The gas-funding message sender is a special case of `MessageSender`, whose iterator of messages is finite
/// and is based on initial test-harness configuration. The gas funding messages consist of transactions that
/// mint and allocate the rollup's gas funding token to all the accounts in the account pool, so that those
/// accounts may take part in broadcasting call messages.
pub async fn get_gas_funding_message_sender<S, Da, R>(
    node_url: &str,
    account_pool: AccountPool<S>,
    serialized_messages_tx: Sender<SerializedPreparedCallMessage>,
    should_stop: Arc<AtomicBool>,
) -> anyhow::Result<Box<dyn MessageSenderT>>
where
    S: Spec,
    Da: DaSpec,
    R: Runtime<S, Da> + sov_modules_api::EncodeCall<Bank<S>> + 'static,
{
    let gas_funding_txs = get_gas_funding_txs(node_url, &account_pool).await?;
    tracing::debug!(
        txs = gas_funding_txs.len(),
        "Gas funding messages have been generated"
    );

    let message_sender: MessageSender<R, Da, S, Bank<S>> = MessageSender::new(
        "gas funding",
        should_stop.clone(),
        Box::new(gas_funding_txs.into_iter()),
        serialized_messages_tx.clone(),
    );
    Ok(Box::new(message_sender))
}
