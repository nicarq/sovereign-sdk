//! Contains a simple client to interact with sovereign rollup node

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Context;
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use futures::StreamExt;
use reqwest::StatusCode;
use sov_api_spec::types;
use sov_bank::utils::TokenHolder;
use sov_bank::{Amount, Coins, TokenId};
use sov_modules_api::prelude::tracing;
use sov_modules_api::rest::utils::ResponseObject;
use sov_rollup_interface::crypto::{CredentialId, PublicKey};
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::zk::CryptoSpec;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct NonceResponse {
    key: CredentialId,
    value: u64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct TokenIdResponse {
    token_id: TokenId,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound = "S::Address: serde::Serialize + serde::de::DeserializeOwned")]
struct AuthorizedMintersResponse<S: sov_modules_api::Spec> {
    authorized_minters: Vec<TokenHolder<S>>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound = "S::Address: serde::Serialize + serde::de::DeserializeOwned")]
struct AllowedSequencerResponse<S: sov_modules_api::Spec> {
    key: <S::Da as DaSpec>::Address,
    value: AllowedSequencer<S>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(bound = "S::Address: serde::Serialize + serde::de::DeserializeOwned")]
pub struct AllowedSequencer<S: sov_modules_api::Spec> {
    pub address: S::Address,
    pub balance: Amount,
}

/// NodeClient is a collection of helper methods that can interact with rollup node via REST API.
#[derive(Debug)]
pub struct NodeClient {
    /// Base URL where runtime, sequencer and ledger routers are mounted.
    pub base_url: String,
    /// Client that used to communicate with Runtime REST API.
    http_client: reqwest::Client,
    /// A [`sov_api_spec::Client`] for communication with the full node endpoints.
    pub client: sov_api_spec::Client,
}

impl NodeClient {
    /// Construct a new NodeClient without verifying that the target url is available and supports
    /// the required functionality.
    pub fn new_unchecked(api_url: &str) -> Self {
        let base_url = api_url.to_string();
        let http_client = reqwest::Client::new();
        let client = sov_api_spec::Client::new(api_url);

        NodeClient {
            base_url,
            http_client,
            client,
        }
    }

    /// Constructor. Implies base url for rollup node.
    pub async fn new(api_url: &str) -> anyhow::Result<Self> {
        let client = NodeClient::new_unchecked(api_url);
        if !check_if_rollup_has_standard_modules(&client.http_client, &client.base_url).await? {
            anyhow::bail!("Rollup does not have standard modules with standard names. Not all functions of sov-cli are available");
        }

        Ok(client)
    }

    /// Simplified constructor for testing.
    pub async fn new_at_localhost(port: u16) -> anyhow::Result<Self> {
        let api_url = format!("http://127.0.0.1:{}", port);
        Self::new(&api_url).await
    }

    /// Fetches the nonce associated with a given public key.
    /// Returns an error if the HTTP request fails or the response cannot be parsed.
    /// If nonce is not found, it will return 0.
    pub async fn get_nonce_for_public_key<S: sov_modules_api::Spec>(
        &self,
        pub_key: &<S::CryptoSpec as CryptoSpec>::PublicKey,
    ) -> anyhow::Result<u64> {
        let credential_id = pub_key.credential_id::<<S::CryptoSpec as CryptoSpec>::Hasher>();
        let nonce_url = format!(
            "{}/modules/nonces/state/nonces/items/{}",
            self.base_url, credential_id
        );
        let response = self.http_client.get(&nonce_url).send().await?;
        let response = response.json::<ResponseObject<NonceResponse>>().await?;

        let nonce = response.data.map(|data| data.value).unwrap_or_default();

        tracing::debug!(url = nonce_url, ?nonce, "Queried nonce");

        Ok(nonce)
    }

    /// Getting [`TokenId`] from given parameters.
    pub async fn get_token_id<S: sov_modules_api::Spec>(
        &self,
        token_name: &str,
        deployer: &S::Address,
    ) -> anyhow::Result<TokenId> {
        let token_url = format!(
            "{}/modules/bank/tokens?token_name={}&sender={}",
            self.base_url, token_name, deployer
        );
        tracing::debug!(url = token_url, "Querying token_id");

        let response = self.http_client.get(token_url).send().await?;
        let response = response.json::<ResponseObject<TokenIdResponse>>().await?;

        let data = response
            .data
            .ok_or_else(|| anyhow::anyhow!("No data in token response"))?;

        Ok(data.token_id)
    }

    async fn query_amount(&self, url: &str) -> anyhow::Result<Amount> {
        let response = self.http_client.get(url).send().await?;
        let response = response.json::<ResponseObject<Coins>>().await?;

        let data = response
            .data
            .ok_or_else(|| anyhow::anyhow!("No data in balance response"))?;
        Ok(data.amount)
    }

    /// Get total supply of given sov-bank token
    pub async fn get_total_supply(&self, token_id: &TokenId) -> anyhow::Result<Amount> {
        let total_supply_url = format!(
            "{}/modules/bank/tokens/{}/total-supply",
            self.base_url, token_id
        );
        tracing::debug!("Querying total supply");

        self.query_amount(&total_supply_url).await
    }

    /// Get list of authorized minters for given token.
    pub async fn get_authorized_minters<S: sov_modules_api::Spec>(
        &self,
        token_id: &TokenId,
    ) -> anyhow::Result<Vec<TokenHolder<S>>> {
        let url = format!(
            "{}/modules/bank/tokens/{}/authorized-minters",
            self.base_url, token_id
        );

        let response = self.http_client.get(url).send().await?;
        let response = response
            .json::<ResponseObject<AuthorizedMintersResponse<S>>>()
            .await?;

        let data = response
            .data
            .ok_or_else(|| anyhow::anyhow!("No data in balance response"))?;

        Ok(data.authorized_minters)
    }

    /// Get balance of the user.
    pub async fn get_balance<S: sov_modules_api::Spec>(
        &self,
        account_address: &S::Address,
        token_id: &TokenId,
        rollup_height: Option<u64>,
    ) -> anyhow::Result<Amount> {
        let height_param: String = rollup_height
            .map(|h| format!("?rollup_height={}", h))
            .unwrap_or_default();
        let balance_url = format!(
            "{}/modules/bank/tokens/{}/balances/{}{}",
            self.base_url, token_id, account_address, height_param
        );
        let amount = self.query_amount(&balance_url).await?;

        tracing::debug!(
            address = %account_address,
            url = balance_url,
            %amount,
            "Queried balance",
        );

        Ok(amount)
    }

    /// Publish batch to the sequencer.
    pub async fn publish_batch(
        &self,
        raw_txs: Vec<Vec<u8>>,
        wait_for_processing: bool,
    ) -> anyhow::Result<()> {
        let response = self
            .client
            .publish_batch(&types::PublishBatchBody {
                transactions: raw_txs
                    .into_iter()
                    .map(|tx| BASE64_STANDARD.encode(tx))
                    .collect(),
            })
            .await
            .context("Unable to publish batch")?;

        let response_data = &response
            .data
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Response data was empty"))?;

        println!(
            "Your batch was submitted to the sequencer for publication. Response: {:?}",
            response_data
        );

        if wait_for_processing {
            // We pick the first tx hash of the batch, any would work.
            let tx_hash_to_wait = response_data.tx_hashes[0].clone();
            let max_waiting_time = Duration::from_secs(300);
            println!(
                "Going to wait for batch to be processed, up to {:?}",
                max_waiting_time
            );
            let start_wait = Instant::now();

            let mut subscription = self
                .client
                .subscribe_to_tx_status_updates(tx_hash_to_wait.parse()?)
                .await?;

            while start_wait.elapsed() < max_waiting_time {
                if let Some(tx_info) = subscription.next().await.transpose()? {
                    if tx_info.status == types::TxStatus::Processed
                        || tx_info.status == types::TxStatus::Finalized
                    {
                        println!("Rollup has processed the submitted batch!");
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

    /// Performs a get request at given URL on the REST API socket.
    pub async fn query_rest_endpoint<R: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> anyhow::Result<R> {
        let url = format!("{}{}", self.base_url, url);
        let response = self.http_client.get(url).send().await?;
        let data = response.json::<R>().await?;
        Ok(data)
    }

    /// HTTP GET to the given endpoint, returning plain text.
    pub async fn http_get(&self, url: &str) -> anyhow::Result<String> {
        let url = format!("{}{}", self.base_url, url);
        Ok(self.http_client.get(url).send().await?.text().await?)
    }

    /// Requests if given DA address is allowed sequencer.
    /// Returns balance as well.
    pub async fn sequencer_rollup_address<S: sov_modules_api::Spec, Da: DaSpec>(
        &self,
        da_address: &Da::Address,
    ) -> anyhow::Result<Option<AllowedSequencer<S>>> {
        let url = format!(
            "{}/modules/sequencer-registry/state/allowed-sequencers/items/{}",
            self.base_url, &da_address,
        );

        let response = self.http_client.get(url).send().await?;
        let response = match response
            .json::<ResponseObject<AllowedSequencerResponse<S>>>()
            .await
        {
            Ok(r) => r,
            Err(err) => {
                if err.status() == Some(StatusCode::NOT_FOUND) {
                    return Ok(None);
                }
                anyhow::bail!(err);
            }
        };

        let allowed_sequencer = response
            .data
            .expect("Data should be set, otherwise HTTP 404");

        Ok(Some(allowed_sequencer.value))
    }
}

#[derive(serde::Deserialize)]
struct ModuleInfo {
    #[allow(dead_code)]
    id: String,
}

#[derive(serde::Deserialize)]
struct ModulesList {
    modules: HashMap<String, ModuleInfo>,
}

/// Call to list of modules endpoint and checking if all modules are listed there.
/// It assumes that "bank", "accounts" and "nonces" are standard Sovereign modules.
async fn check_if_rollup_has_standard_modules(
    client: &reqwest::Client,
    base_url: &str,
) -> anyhow::Result<bool> {
    let url = format!("{}/modules", base_url);
    let response = client.get(&url).send().await?;
    let response_json: ResponseObject<ModulesList> = response.json().await?;
    let module_response = response_json
        .data
        .ok_or(anyhow::anyhow!("List of modules is missing"))?;

    Ok(module_response.modules.contains_key("bank")
        && module_response.modules.contains_key("accounts")
        && module_response.modules.contains_key("nonces")
        && module_response.modules.contains_key("sequencer-registry"))
}
