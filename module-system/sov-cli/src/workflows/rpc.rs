//! Query the current state of the rollup and send transactions

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use borsh::{BorshDeserialize, BorshSerialize};
use jsonrpsee::core::client::{ClientT, Error};
use jsonrpsee::http_client::HttpClientBuilder;
use jsonrpsee::tokio::time::sleep;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sov_accounts::AccountsRpcClient;
use sov_bank::{BalanceResponse, BankRpcClient, TokenId};
use sov_modules_api::{clap, CryptoSpec, PublicKey};
use sov_rollup_interface::digest::Digest;

use crate::wallet_state::{AddressEntry, KeyIdentifier, WalletState};
use crate::workflows::keys::load_key;
use crate::workflows::NO_ACCOUNTS_FOUND;

const BAD_RPC_URL: &str = "Unable to connect to provided rpc. You can change to a different rpc url with the `rpc set-url` subcommand ";

/// Query the current state of the rollup and send transactions
#[derive(clap::Subcommand)]
pub enum RpcWorkflows<S: sov_modules_api::Spec> {
    /// Set the url of the rpc server to use
    SetUrl {
        /// A url like http://localhost:8545
        rpc_url: String,
    },
    /// Query the RPC server for the nonce of the provided account. If no account is provided, the active account is used
    GetNonce {
        /// (Optional) The account to query the nonce for (default: the active account)
        #[clap(subcommand)]
        account: Option<KeyIdentifier<S>>,
    },
    /// Query the address of token by name, salt and owner
    GetTokenAddress {
        /// The name of the token to query for
        token_name: String,
        /// The deployer of the token.
        /// In the case of genesis token, it can be looked up in genesis config JSON.
        /// Check the server logs if it does not match.
        deployer_address: S::Address,
        /// A salt used in the token ID derivation.
        salt: u64,
    },
    /// Query the rpc server for the token balance of an account
    GetBalance {
        /// (Optional) The account to query the balance of (default: the active account)
        #[clap(subcommand)]
        account: Option<KeyIdentifier<S>>,
        /// The ID of the token to query for
        token_id: TokenId,
    },
    /// Sign all transactions from the current batch and submit them to the rollup.
    /// Nonces will be set automatically.
    SubmitBatch {
        /// (Optional) The account to sign transactions for this batch (default: the active account)
        #[clap(subcommand)]
        account: Option<KeyIdentifier<S>>,
        /// (Optional) The nonce to use for the first transaction in the batch (default: the current nonce for the account). Any other transactions will
        /// be signed with sequential nonces starting from this value.
        nonce_override: Option<u64>,
    },
}

impl<S: sov_modules_api::Spec> RpcWorkflows<S> {
    fn resolve_account<'wallet, Tx>(
        &self,
        wallet_state: &'wallet mut WalletState<Tx, S>,
    ) -> Result<&'wallet AddressEntry<S>, anyhow::Error>
    where
        Tx: Serialize + DeserializeOwned + BorshSerialize + BorshDeserialize,
    {
        let account_id = match self {
            RpcWorkflows::SetUrl { .. } | RpcWorkflows::GetTokenAddress { .. } => None,
            RpcWorkflows::GetNonce { account }
            | RpcWorkflows::GetBalance { account, .. }
            | RpcWorkflows::SubmitBatch { account, .. } => account.as_ref(),
        };

        let account = if let Some(id) = account_id {
            let addr = wallet_state.addresses.get_address(id);

            addr.ok_or_else(|| anyhow::format_err!("No account found matching identifier: {}", id))?
        } else {
            wallet_state
                .addresses
                .default_address()
                .ok_or_else(|| anyhow::format_err!(NO_ACCOUNTS_FOUND))?
        };
        Ok(account)
    }
}

impl<S: sov_modules_api::Spec + Serialize + DeserializeOwned + Send + Sync> RpcWorkflows<S> {
    /// Run the rpc workflow
    pub async fn run<Tx>(
        &self,
        wallet_state: &mut WalletState<Tx, S>,
        _app_dir: impl AsRef<Path>,
    ) -> Result<(), anyhow::Error>
    where
        Tx: Serialize + DeserializeOwned + BorshSerialize + BorshDeserialize,
    {
        // If the user is just setting the RPC url, we can skip the usual setup
        if let RpcWorkflows::SetUrl { rpc_url } = self {
            let _client = HttpClientBuilder::default()
                .build(rpc_url)
                .context("Invalid RPC url: ")?;
            wallet_state.rpc_url = Some(rpc_url.clone());
            println!("Set RPC url to {}", rpc_url);
            return Ok(());
        }

        // Otherwise, we need to initialize an  RPC and resolve the active account
        let rpc_url = wallet_state
            .rpc_url
            .as_ref()
            .ok_or(anyhow::format_err!(
                "No RPC url set. Use the `rpc set-url` subcommand to set one"
            ))?
            .clone();
        let client = HttpClientBuilder::default().build(&rpc_url)?;

        let wait_timeout = Duration::from_millis(500);
        // 120 * 500ms = 60s
        let attempts = 120;
        for attempt_number in 0..attempts {
            // Calling some non-existing method, at least we should get HTTP response
            let response = client.request::<(), [u8; 0]>("health", []).await;

            if let Err(Error::Transport(_)) = response {
                if attempt_number > 3 {
                    println!(
                        "RPC endpoint {} is not responding, will wait for {:?}...",
                        &rpc_url, wait_timeout
                    );
                }
                sleep(wait_timeout).await;
                continue;
            }
            break;
        }

        let account = self.resolve_account(wallet_state)?;

        // Finally, run the workflow
        match self {
            RpcWorkflows::SetUrl { .. } => {
                unreachable!("This case was handled above")
            }
            RpcWorkflows::GetNonce { .. } => {
                let nonce = get_nonce_for_account(&client, account).await?;
                println!("Nonce for account {} is {}", account.address, nonce);
            }
            RpcWorkflows::GetBalance {
                account: _,
                token_id,
            } => {
                let BalanceResponse { amount } = BankRpcClient::<S>::balance_of(
                    &client,
                    None,
                    account.address.clone(),
                    *token_id,
                )
                .await
                .context(BAD_RPC_URL)?;

                println!(
                    "Balance for account {} is {}",
                    account.address,
                    amount.unwrap_or_default()
                );
            }
            RpcWorkflows::SubmitBatch { nonce_override, .. } => {
                let private_key = load_key::<S>(&account.location).with_context(|| {
                    format!("Unable to load key {}", account.location.display())
                })?;

                let nonce = match nonce_override {
                    Some(nonce) => *nonce,
                    None => get_nonce_for_account(&client, account).await?,
                };

                let txs = wallet_state.take_signed_transactions(&private_key, nonce);

                for (i, tx) in txs.iter().enumerate() {
                    let tx_hash = hex::encode(<S::CryptoSpec as CryptoSpec>::Hasher::digest(tx));
                    println!("Submitting tx: {}: {}", i, tx_hash);
                    let request = serde_json::json!({ "body": tx });
                    let response: serde_json::Value = client
                        .request("sequencer_acceptTx", [request])
                        .await
                        .context("Unable to submit transaction")?;
                    println!("Transaction {} has been submitted: {:?}", tx_hash, response);
                }
                println!("Triggering batch publishing");

                let response: serde_json::Value = client
                    .request("sequencer_publishBatch", txs.clone())
                    .await
                    .context("Unable to publish batch")?;

                // Print the result
                println!(
                    "Your batch was submitted to the sequencer for publication. Response: {:?}",
                    response
                );
            }
            RpcWorkflows::GetTokenAddress {
                token_name,
                deployer_address: owner_address,
                salt,
                ..
            } => {
                let address = BankRpcClient::<S>::token_id(
                    &client,
                    token_name.clone(),
                    owner_address.clone(),
                    *salt,
                )
                .await
                .context(BAD_RPC_URL)?;

                println!("Address of token {} is {}", token_name, address);
            }
        }
        Ok(())
    }
}

async fn get_nonce_for_account<S: sov_modules_api::Spec + Send + Sync + Serialize>(
    client: &(impl ClientT + Send + Sync),
    account: &AddressEntry<S>,
) -> Result<u64, anyhow::Error> {
    let credential_id = account
        .pub_key
        .credential_id::<<S::CryptoSpec as CryptoSpec>::Hasher>();

    Ok(match AccountsRpcClient::<S>::get_account(
        client,
        credential_id,
    )
    .await
    .context(
        "Unable to connect to provided RPC. You can change to a different RPC url with the `rpc set-url` subcommand ",
    )? {
        sov_accounts::Response::AccountExists { addr: _, nonce } => nonce,
        _ => 0,
    })
}
