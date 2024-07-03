use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use jsonrpsee::core::client::ClientT;
use sov_bank::{Amount, Bank, BankRpcClient, CallMessage, GAS_TOKEN_ID};
use sov_modules_api::Spec;
use sov_rollup_interface::crypto::PrivateKey;
use sov_rollup_interface::zk::CryptoSpec;

use crate::account_pool::AccountPool;
use crate::types::PreparedCallMessage;

const DEFAULT_MAX_FEE: u64 = 10_000_000;
// How much funds account should have to be considered a "whale".
const MINIMAL_WHALE_BALANCE: u64 = 100_000_000;
const MAX_MINT_BATCH_SIZE: usize = 10;

fn get_bank_config<S: Spec>(
    genesis_dir: impl AsRef<Path>,
) -> anyhow::Result<sov_bank::BankConfig<S>> {
    let path = genesis_dir.as_ref().join("bank.json");

    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let bank_config = serde_json::from_reader(reader)?;

    Ok(bank_config)
}

/// Setup returns batched transactions, because it needs to have control on how big batch is going to be.
pub async fn setup<S: Spec>(
    account_pool: &AccountPool<S>,
    genesis_dir: impl AsRef<Path>,
    client: &(impl ClientT + Send + Sync),
    max_batch_size: usize,
) -> anyhow::Result<Vec<Vec<PreparedCallMessage<S, Bank<S>>>>> {
    let mut prepared_batches: Vec<_> = Vec::new();
    let bank_config = get_bank_config::<S>(genesis_dir)?;
    tracing::info!(?bank_config, "Bank config");

    let gas_whales: Vec<S::Address> = bank_config
        .gas_token_config
        .address_and_balances
        .into_iter()
        .filter_map(|(addr, balance)| {
            if balance < MINIMAL_WHALE_BALANCE {
                return None;
            }
            if account_pool.contains_key(&addr) {
                Some(addr)
            } else {
                None
            }
        })
        .collect();

    if gas_whales.is_empty() {
        anyhow::bail!("No whales found!");
    }
    tracing::info!(?gas_whales, "Gas whales");

    let total_supply = BankRpcClient::<S>::supply_of(client, None, GAS_TOKEN_ID)
        .await?
        .amount
        .expect("Gas token should exist");

    if total_supply < (Amount::MAX / 2) {
        let gas_token_minter = bank_config
            .gas_token_config
            .authorized_minters
            .iter()
            .find(|addr| account_pool.contains_key(addr))
            .expect("Haven't found gas token minter in available keys. Cannot proceed");
        let max_mint_batch_size = std::cmp::min(max_batch_size, MAX_MINT_BATCH_SIZE);
        tracing::info!("Total supply of gas token is not large enough, need to mint!");
        let to_mint = Amount::MAX - 100 - total_supply;
        let to_mint_per_whale = to_mint / gas_whales.len() as u64;
        let mut mint_batch = Vec::new();
        for whale_address in &gas_whales {
            tracing::info!(amount = to_mint_per_whale, to = %whale_address, from = %gas_token_minter, "Mint call message");
            let call_message = CallMessage::<S>::Mint {
                coins: sov_bank::Coins {
                    amount: to_mint_per_whale,
                    token_id: GAS_TOKEN_ID,
                },
                mint_to_address: whale_address.clone(),
            };
            mint_batch.push(PreparedCallMessage::<S, Bank<S>>::new(
                call_message,
                gas_token_minter.clone(),
                DEFAULT_MAX_FEE * 3,
            ));
            if mint_batch.len() == max_mint_batch_size {
                prepared_batches.push(mint_batch);
                mint_batch = Vec::new();
            }
        }
        if !mint_batch.is_empty() {
            prepared_batches.push(mint_batch);
        }
    }

    let accounts_to_fill = account_pool
        .addresses()
        .filter(|addr| !gas_whales.contains(addr));

    tracing::info!("Filling gas balance for all non-whale accounts in account pool");
    let mut fill_batch = Vec::new();
    for (idx, account) in accounts_to_fill.enumerate() {
        let whale_idx = idx % gas_whales.len();
        let whale = &gas_whales[whale_idx];

        // TODO: Better calculation on how much gas will be needed for account.
        let amount = 1_000_000;

        let call_message = CallMessage::<S>::Transfer {
            to: account.clone(),
            coins: sov_bank::Coins {
                amount,
                token_id: GAS_TOKEN_ID,
            },
        };

        fill_batch.push(PreparedCallMessage::<S, Bank<S>>::new(
            call_message,
            whale.clone(),
            DEFAULT_MAX_FEE,
        ));
        if fill_batch.len() == max_batch_size {
            prepared_batches.push(fill_batch);
            fill_batch = Vec::new();
        }
    }

    if !fill_batch.is_empty() {
        prepared_batches.push(fill_batch);
    }

    Ok(prepared_batches)
}

/// Generate "regular" testing call messages
pub fn generate_bank_transfer_messages<S: Spec>(
    account_pool: &AccountPool<S>,
    total_txs: u64,
) -> anyhow::Result<Vec<PreparedCallMessage<S, Bank<S>>>> {
    let mut prepared_call_messages: Vec<_> = Vec::with_capacity(total_txs as usize);
    tracing::info!("Generating bank transfers to non-existing users");
    for (_, from) in (0..total_txs).zip(account_pool.cycle_over_all()) {
        let to_address =
            (&<<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey::generate().pub_key()).into();
        let call_message = CallMessage::<S>::Transfer {
            to: to_address,
            coins: sov_bank::Coins {
                amount: 1,
                token_id: GAS_TOKEN_ID,
            },
        };
        prepared_call_messages.push(PreparedCallMessage::new(
            call_message,
            from.clone(),
            DEFAULT_MAX_FEE,
        ));
    }

    // TODO: More scenarios, where each user creates token and mints some, and sends to some existing users

    Ok(prepared_call_messages)
}

pub fn generate_token_contract_creation_messages<S: Spec>(
    account_pool: &AccountPool<S>,
    num_contracts: u64,
) -> anyhow::Result<Vec<PreparedCallMessage<S, Bank<S>>>> {
    tracing::info!("Generating {num_contracts} token contract creation transactions");

    Ok((0..num_contracts)
        .zip(account_pool.cycle_over_all())
        .map(|(i, token_creator_address)| {
            PreparedCallMessage::new(
                CallMessage::<S>::CreateToken {
                    salt: i,
                    initial_balance: u64::MAX,
                    token_name: format!("token_{i}"),
                    mint_to_address: token_creator_address.clone(),
                    authorized_minters: vec![token_creator_address.clone()],
                },
                token_creator_address.clone(),
                DEFAULT_MAX_FEE,
            )
        })
        .collect::<Vec<_>>())
}
