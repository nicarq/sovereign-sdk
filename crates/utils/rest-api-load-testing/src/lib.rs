#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
mod report;
pub use report::*;

mod req_sender;
mod scenario;
pub use req_sender::*;
use scenario::{SimpleScenario, TestScenario};

/// Starts the measurement process.
pub async fn start(requests: Requests) -> Report {
    let test_plan = SimpleScenario::new();
    test_plan.start_experiment(requests).await
}
