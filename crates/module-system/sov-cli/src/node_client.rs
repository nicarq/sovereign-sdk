//! Contains a simple client to interact with sovereign rollup node

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Context;
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use futures::StreamExt;
use reqwest::ClientBuilder;
use sov_bank::utils::TokenHolder;
use sov_bank::{Amount, Coins, TokenId};
use sov_ledger_json_client::Client as LedgerClient;
use sov_modules_api::prelude::tracing;
use sov_modules_api::rest::utils::ResponseObject;
use sov_rollup_interface::crypto::{CredentialId, PublicKey};
use sov_rollup_interface::zk::CryptoSpec;
use sov_sequencer_json_client::types;

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

/// NodeClient is a collection of helper methods that can interact with rollup node via REST API.
pub struct NodeClient {
    base_url: String,
    http_client: reqwest::Client,
    ledger_client: LedgerClient,
    sequencer_client: sov_sequencer_json_client::Client,
}

impl NodeClient {
    /// Constructor. Implies base url for rollup node, as sequencer and ledger are appended.
    pub async fn new(api_url: &str) -> anyhow::Result<Self> {
        let base_url = api_url.to_string();
        let http_client = ClientBuilder::default()
            .build()
            .map_err(|e| anyhow::anyhow!(e))?;
        if !check_if_rollup_has_standard_modules(&http_client, &base_url).await? {
            anyhow::bail!("Rollup does not have standard modules with standard names. Not all functions of sov-cli are available");
        }
        let ledger_url = format!("{}/ledger", api_url);
        let ledger_client = LedgerClient::new(&ledger_url);

        let sequencer_url = format!("{}/sequencer", api_url);
        let sequencer_client = sov_sequencer_json_client::Client::new(&sequencer_url);

        Ok(NodeClient {
            base_url,
            http_client,
            ledger_client,
            sequencer_client,
        })
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
        salt: u64,
        deployer: &S::Address,
    ) -> anyhow::Result<TokenId> {
        let token_url = format!(
            "{}/modules/bank/tokens?token_name={}&salt={}&sender={}",
            self.base_url, token_name, salt, deployer
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
    ) -> anyhow::Result<Amount> {
        let balance_url = format!(
            "{}/modules/bank/tokens/{}/balances/{}",
            self.base_url, token_id, account_address
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
        && module_response.modules.contains_key("nonces"))
}
