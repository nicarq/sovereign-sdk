//! Workflows for interacting with Rollup node API
use std::path::Path;

use anyhow::Context;
use borsh::{BorshDeserialize, BorshSerialize};
use reqwest::Url;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sov_bank::TokenId;
use sov_modules_api::clap;

use crate::wallet_state::{KeyIdentifier, WalletState};
use crate::workflows::keys::load_key;

/// Query the current state of the rollup and send transactions.
#[derive(clap::Subcommand)]
pub enum NodeWorkflows<S: sov_modules_api::Spec> {
    /// Set URL to the rollup node.
    SetUrl {
        /// A URL to the REST API root endpoint http://localhost:12346
        url: String,
    },
    /// Query rollup node for the nonce of the provided account. If no account is provided, the active account is used.
    GetNonce {
        /// (Optional) The account to query the nonce for (default: the active account)
        #[clap(subcommand)]
        account: Option<KeyIdentifier<S>>,
    },
    /// Query the address of token by name, salt and owner
    FindTokenId {
        /// The name of the token to query for
        token_name: String,
        /// The deployer of the token.
        /// In the case of genesis token, it can be looked up in genesis config JSON.
        /// Check the server logs if it does not match.
        deployer_address: S::Address,
        /// A salt used in the token ID derivation.
        salt: u64,
    },
    /// Query the rollup nod for the token balance of an account
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
        /// (Optional) Waits for a given batch to be processed by the rollup node.
        #[arg(short, long)]
        wait_for_processing: bool,
        /// (Optional) The nonce to use for the first transaction in the batch (default: the current nonce for the account). Any other transactions will
        /// be signed with sequential nonces starting from this value.
        nonce_override: Option<u64>,
    },
}

impl<S: sov_modules_api::Spec + Serialize + DeserializeOwned> NodeWorkflows<S> {
    /// Runs API workflow.
    pub async fn run<Tx>(
        &self,
        wallet_state: &mut WalletState<Tx, S>,
        _app_dir: impl AsRef<Path>,
    ) -> anyhow::Result<()>
    where
        Tx: Serialize + DeserializeOwned + BorshSerialize + BorshDeserialize,
    {
        if let Self::SetUrl { url } = self {
            Url::parse(url).map_err(|e| anyhow::anyhow!("Failed to parse API URL: {:?}", e))?;
            let prev_url = wallet_state.rest_api_url.clone();
            wallet_state.rest_api_url = Some(url.clone());
            println!("Set REST API URL from {:?} to {}", prev_url, url);
            return Ok(());
        }

        let url = wallet_state
            .rest_api_url
            .as_ref()
            .ok_or(anyhow::format_err!(
                "No REST API URL set. Use the `api set-url` subcommand to set one"
            ))?
            .clone();

        let api_client = client::SimpleApiClient::new(&url)?;

        match self {
            NodeWorkflows::SetUrl { .. } => {
                unreachable!("This case was handled above")
            }
            NodeWorkflows::GetNonce { account } => {
                let account = wallet_state.resolve_account(account.as_ref())?;
                let nonce = api_client.get_nonce_for_account(account).await?;
                println!("Nonce for account {} is {}", account.address, nonce);
            }
            NodeWorkflows::FindTokenId {
                token_name,
                deployer_address,
                salt,
            } => {
                let token_id = api_client
                    .get_token_id::<S>(token_name, *salt, deployer_address)
                    .await?;
                println!("Id of token {} is {}", token_name, token_id);
            }
            NodeWorkflows::GetBalance { account, token_id } => {
                let account = wallet_state.resolve_account(account.as_ref())?;
                let balance = api_client
                    .get_balance::<S>(&account.address, token_id)
                    .await?;
                println!(
                    "Balance of token {} for account {} is {}",
                    token_id, account.address, balance
                );

                return Ok(());
            }
            NodeWorkflows::SubmitBatch {
                account,
                wait_for_processing,
                nonce_override,
            } => {
                let account = wallet_state.resolve_account(account.as_ref())?;
                let private_key = load_key::<S>(&account.location).with_context(|| {
                    format!("Unable to load key {}", account.location.display())
                })?;

                let nonce = match nonce_override {
                    Some(nonce) => *nonce,
                    None => api_client.get_nonce_for_account(account).await?,
                };

                let txs = wallet_state.take_signed_transactions(&private_key, nonce);

                api_client.publish_batch(txs, *wait_for_processing).await?;
            }
        }

        Ok(())
    }
}

mod client {
    use std::time::{Duration, Instant};

    use anyhow::Context;
    use base64::prelude::BASE64_STANDARD;
    use base64::Engine;
    use futures::StreamExt;
    use reqwest::ClientBuilder;
    use sov_bank::{Amount, Coins, TokenId};
    use sov_ledger_json_client::Client as LedgerClient;
    use sov_modules_api::rest::utils::ResponseObject;
    use sov_rollup_interface::crypto::{CredentialId, PublicKey};
    use sov_rollup_interface::zk::CryptoSpec;
    use sov_sequencer_json_client::types;

    use crate::wallet_state::AddressEntry;

    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    struct NonceResponse {
        key: CredentialId,
        value: Option<u64>,
    }

    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    struct TokenIdResponse {
        token_id: TokenId,
    }

    pub struct SimpleApiClient {
        base_url: String,
        http_client: reqwest::Client,
        ledger_client: LedgerClient,
        sequencer_client: sov_sequencer_json_client::Client,
    }

    impl SimpleApiClient {
        pub fn new(api_url: &str) -> anyhow::Result<Self> {
            let base_url = api_url.to_string();
            let http_client = ClientBuilder::default()
                .build()
                .map_err(|e| anyhow::anyhow!(e))?;
            let ledger_url = format!("{}/ledger", api_url);
            let ledger_client = LedgerClient::new(&ledger_url);

            let sequencer_url = format!("{}/sequencer", api_url);
            let sequencer_client = sov_sequencer_json_client::Client::new(&sequencer_url);

            Ok(SimpleApiClient {
                base_url,
                http_client,
                ledger_client,
                sequencer_client,
            })
        }

        pub async fn get_nonce_for_account<S: sov_modules_api::Spec>(
            &self,
            account: &AddressEntry<S>,
        ) -> anyhow::Result<u64> {
            let credential_id = account
                .pub_key
                .credential_id::<<S::CryptoSpec as CryptoSpec>::Hasher>();
            let nonce_url = format!(
                "{}/modules/nonces/state/nonces/items/{}",
                self.base_url, credential_id
            );
            println!("Querying nonce from {}", nonce_url);

            let response = self.http_client.get(nonce_url).send().await?;
            let response = response.json::<ResponseObject<NonceResponse>>().await?;

            let data = response
                .data
                .ok_or_else(|| anyhow::anyhow!("No data in nonce response"))?;

            Ok(data.value.unwrap_or_default())
        }

        pub async fn get_token_id<S: sov_modules_api::Spec>(
            &self,
            token_name: &str,
            salt: u64,
            deployer: &S::Address,
        ) -> anyhow::Result<TokenId> {
            let token_url = format!(
                "{}/modules/bank/tokens?token_name={}&salt={}&sender={}",
                self.base_url, token_name, salt, deployer
            );
            println!("Querying token_id from {}", token_url);

            let response = self.http_client.get(token_url).send().await?;
            let response = response.json::<ResponseObject<TokenIdResponse>>().await?;

            let data = response
                .data
                .ok_or_else(|| anyhow::anyhow!("No data in token response"))?;

            Ok(data.token_id)
        }

        pub async fn get_balance<S: sov_modules_api::Spec>(
            &self,
            account_address: &S::Address,
            token_id: &TokenId,
        ) -> anyhow::Result<Amount> {
            let balance_url = format!(
                "{}/modules/bank/tokens/{}/balances/{}",
                self.base_url, token_id, account_address
            );
            println!(
                "Querying balance for account {} at {}",
                account_address, balance_url
            );

            let response = self.http_client.get(balance_url).send().await?;
            let response = response.json::<ResponseObject<Coins>>().await?;

            let data = response
                .data
                .ok_or_else(|| anyhow::anyhow!("No data in balance response"))?;
            Ok(data.amount)
        }

        pub async fn publish_batch(
            &self,
            raw_txs: Vec<Vec<u8>>,
            wait_for_processing: bool,
        ) -> anyhow::Result<()> {
            let response = self
                .sequencer_client
                .publish_batch(&types::PublishBatchBody {
                    transactions: raw_txs
                        .into_iter()
                        .map(|tx| BASE64_STANDARD.encode(tx))
                        .collect(),
                })
                .await
                .context("Unable to publish batch")?;

            let response_data = response
                .data
                .as_ref()
                .ok_or(anyhow::anyhow!("No data in response"))?;

            println!(
                "Your batch was submitted to the sequencer for publication. Response: {:?}",
                response_data
            );

            if wait_for_processing {
                let target_da_height: u64 = response_data
                    .da_height
                    .try_into()
                    .expect("da_height is out of range");
                let max_waiting_time = Duration::from_secs(300);
                println!(
                    "Going to wait for target slot number {} to be processed, up to {:?}",
                    target_da_height, max_waiting_time
                );
                let start_wait = Instant::now();

                // Subscribe to slots only to check our batch if the slot has been published.
                let mut slot_subscription = self.ledger_client.subscribe_slots().await?;

                while start_wait.elapsed() < max_waiting_time {
                    if let Some(latest_slot) = slot_subscription.next().await.transpose()? {
                        if latest_slot.number >= target_da_height {
                            println!(
                                "Rollup has processed target DA height={}!",
                                target_da_height
                            );
                            return Ok(());
                        }
                    }
                }
                anyhow::bail!(
                    "Giving up waiting for target batch to be published after {:?}",
                    start_wait.elapsed()
                );
            }
            Ok(())
        }
    }
}
