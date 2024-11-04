use std::time::Duration;

use reqwest::{Client, ClientBuilder};

const REQUEST_TIMEOUT: Duration = std::time::Duration::from_secs(10);

#[derive(Debug)]
/// The output of a single HTTP request.
pub struct ResponseOutput {
    #[allow(dead_code)]
    pub(crate) status: u16,
}

/// The collection of urls to be analyzed
pub struct Requests {
    pub(crate) urls: Vec<String>,
}

impl Requests {
    /// Creates `Requests` from a list of endpoints and a host.
    pub fn new(host: &str, endpoints: Vec<&'static str>) -> Self {
        Self {
            urls: endpoints
                .into_iter()
                .map(|e| format!("{host}/{e}"))
                .collect(),
        }
    }
}

pub(crate) struct RequestSender {
    client: Client,
}

impl RequestSender {
    pub(crate) fn new() -> Self {
        let client = ClientBuilder::new()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap();
        Self { client }
    }

    pub(crate) async fn request(&self, url: &str) -> anyhow::Result<ResponseOutput> {
        let response = self.client.get(url).send().await?;

        Ok(ResponseOutput {
            status: response.status().as_u16(),
        })
    }
}
