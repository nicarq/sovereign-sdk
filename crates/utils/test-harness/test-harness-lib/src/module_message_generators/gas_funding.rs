use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use derive_getters::Getters;
use derive_more::Constructor;
use jsonrpsee::http_client::HttpClientBuilder;
use sov_bank::{Amount, Bank, BankRpcClient, CallMessage, GAS_TOKEN_ID};
use sov_modules_api::prelude::tokio;
use sov_modules_api::Spec;
use sov_rollup_interface::services::da::DaService;
use tokio::sync::mpsc::Sender;

use super::{MessageSender, MessageSenderT};
use crate::account_pool::AccountPool;
use crate::constants::DEFAULT_MAX_FEE;
use crate::{get_bank_config, PreparedCallMessage, SerializedPreparedCallMessage};

// How much funds account should have to be considered a "whale".
const MINIMAL_WHALE_BALANCE: u64 = 5_000_000;

/// [`GasFundingConfig`] holds the values required to create gas funding transactions,
/// which mint and transfer the rollup's gas token to accounts in the [`crate::AccountPool`].
#[derive(Clone, Debug, Constructor, Getters)]
pub struct GasFundingConfig {
    /// This is use to create an client in order to query the rollup for account balances,
    /// nonces etc.
    rpc_url: String,

    /// The genesis directory contains information pertaining to the genesis state of the
    /// rollup, including initial allocations of the rollup's native gas token.
    genesis_dir: String,
}

/// This function creates the call messages required to mint and distribute the rollup's gas-token -
/// as defined in the genesis config - to the set of accounts in the account pool.
pub async fn get_gas_funding_txs<S: Spec>(
    gas_funding_config: GasFundingConfig,
    account_pool: &AccountPool<S>,
) -> anyhow::Result<Vec<PreparedCallMessage<S, Bank<S>>>> {
    let client = HttpClientBuilder::default().build(gas_funding_config.rpc_url())?;

    let bank_config = get_bank_config::<S>(gas_funding_config.genesis_dir())?;
    tracing::info!(?bank_config, "Bank config");

    let gas_whale_account_pool_indices = {
        let mut map = HashMap::<S::Address, u64>::new();
        bank_config
            .gas_token_config
            .address_and_balances
            .into_iter()
            .for_each(|(address, balance)| {
                if balance >= MINIMAL_WHALE_BALANCE {
                    if let Some(index) = account_pool.get_index(&address) {
                        map.insert(address, *index);
                    } else {
                        tracing::warn!(account = %address, "Account from bank config is not in account pool");
                    }
                }
            });
        map
    };

    let num_gas_whales = gas_whale_account_pool_indices.keys().len();

    if num_gas_whales == 0 {
        anyhow::bail!("No whales found!");
    }

    let total_supply = BankRpcClient::<S>::supply_of(&client, None, GAS_TOKEN_ID)
        .await?
        .amount
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
        for whale_address in gas_whale_account_pool_indices.keys() {
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
        let whales: Vec<S::Address> = gas_whale_account_pool_indices.keys().cloned().collect();
        let whale_idx = idx % num_gas_whales;
        let whale = &whales[whale_idx];
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
pub async fn get_gas_funding_message_sender<S: Spec, Da: DaService>(
    genesis_dir: String,
    rpc_url: String,
    account_pool: AccountPool<S>,
    serialized_messages_tx: Sender<SerializedPreparedCallMessage>,
    should_stop: Arc<AtomicBool>,
) -> anyhow::Result<Box<dyn MessageSenderT>> {
    let gas_funding_txs =
        get_gas_funding_txs(GasFundingConfig::new(rpc_url, genesis_dir), &account_pool).await?;

    let message_sender: MessageSender<
        demo_stf::runtime::Runtime<S, <Da as DaService>::Spec>,
        <Da as DaService>::Spec,
        S,
        Bank<S>,
    > = MessageSender::new(
        "gas funding",
        should_stop.clone(),
        Box::new(gas_funding_txs.into_iter()),
        serialized_messages_tx.clone(),
    );

    Ok(Box::new(message_sender))
}
