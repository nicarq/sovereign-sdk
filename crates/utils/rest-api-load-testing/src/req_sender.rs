use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client, ClientBuilder};

const REQUEST_TIMEOUT: Duration = std::time::Duration::from_secs(10);

#[derive(Debug)]
/// The output of a single HTTP request.
pub struct ResponseOutput {
    #[allow(dead_code)]
    pub(crate) status: u16,
    #[allow(dead_code)]
    pub(crate) body_size: usize,
}

/// The collection of urls to be analyzed
pub struct Requests {
    pub(crate) urls: Vec<Arc<String>>,
}

impl Requests {
    /// Creates `Requests` from a list of endpoints and a host.
    pub fn new(host: &str, endpoints: Vec<&'static str>) -> Self {
        Self {
            urls: endpoints
                .into_iter()
                .map(|e| Arc::new(format!("{host}/{e}")))
                .collect(),
        }
    }
}

#[derive(Clone)]
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
        let status = response.status().as_u16();
        let body = response.bytes().await?;

        Ok(ResponseOutput {
            status,
            body_size: body.len(),
        })
    }
}
