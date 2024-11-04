use std::time::Instant;

use crate::{Measurement, Report, RequestSender, Requests};

/// A test scenario controls the number of concurrent tasks sending messages,
/// the frequency, and other related parameters.
pub trait TestScenario {
    /// Sends requests and creates a report.
    async fn start_experiment(&self, requests: Requests) -> Report;
}

/// A simple test scenario that sends requests in a loop.
pub(crate) struct SimpleScenario {
    request_sender: RequestSender,
}

impl SimpleScenario {
    pub fn new() -> Self {
        Self {
            request_sender: RequestSender::new(),
        }
    }
}

impl TestScenario for SimpleScenario {
    async fn start_experiment(&self, requests: Requests) -> Report {
        let mut measurements = vec![];
        for url in &requests.urls {
            let m = measurement(&self.request_sender, url).await;
            measurements.push(m);
        }
        Report { measurements }
    }
}

async fn measurement(request_sender: &RequestSender, url: &str) -> Measurement {
    let started = Instant::now();
    let output = request_sender.request(url).await;

    Measurement {
        time: started.elapsed().as_nanos(),
        output,
    }
}
