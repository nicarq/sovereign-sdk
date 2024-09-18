use jsonrpsee::ws_client::{WsClient, WsClientBuilder};
use reqwest::{Client, ClientBuilder};
use serde::de::DeserializeOwned;

/// API clients for a running Sovereign rollup node.
///
/// Whenever you need to interact with the node over the network within tests,
/// you can use this.
///
///
/// # Example
///
/// ```
/// use sov_test_utils::ApiClient;
///
/// async fn api_client() -> anyhow::Result<ApiClient> {
///     ApiClient::new(12345, 12346).await
/// }
/// ```
#[derive(Debug)]
pub struct ApiClient {
    /// A [`sov_sequencer_json_client::Client`] for communication with the sequencer.
    pub sequencer: sov_sequencer_json_client::Client,
    /// A [`sov_ledger_json_client::Client`] for communication with the ledger.
    pub ledger: sov_ledger_json_client::Client,
    /// A [`WsClient`] client for communications with RPC.
    pub rpc: WsClient,

    /// This is temporary while REST clients for each module appear.
    rest_port: u16,
    /// [`reqwest::Client`] client as temporary sub for JSON client
    pub raw_rest: Client,
}

impl ApiClient {
    /// Creates a new [`ApiClient`] from the given RPC and REST ports.
    pub async fn new(rpc_port: u16, rest_port: u16) -> anyhow::Result<Self> {
        let sequencer = sov_sequencer_json_client::Client::new(&format!(
            "http://127.0.0.1:{rest_port}/sequencer"
        ));
        let ledger =
            sov_ledger_json_client::Client::new(&format!("http://127.0.0.1:{rest_port}/ledger"));

        let rpc = WsClientBuilder::default()
            .build(&format!("ws://127.0.0.1:{rpc_port}"))
            .await?;

        let raw_rest = ClientBuilder::default().build()?;

        Ok(Self {
            sequencer,
            ledger,
            rpc,
            rest_port,
            raw_rest,
        })
    }

    /// Performs a get request at given URL on the REST API socket.
    pub async fn query_rest_endpoint<R: DeserializeOwned>(&self, url: &str) -> anyhow::Result<R> {
        let url = format!("http://127.0.0.1:{}{}", self.rest_port, url);
        let response = self.raw_rest.get(url).send().await?;
        let data = response.json::<R>().await?;
        Ok(data)
    }

    /// HTTP GET to the given endpoint, returning plain text.
    pub async fn http_get(&self, url: &str) -> anyhow::Result<String> {
        let url = format!("http://127.0.0.1:{}{}", self.rest_port, url);
        Ok(self.raw_rest.get(url).send().await?.text().await?)
    }
}
