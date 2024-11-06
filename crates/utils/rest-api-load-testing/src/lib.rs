#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
mod report;
pub use report::*;

mod req_sender;
mod scenario;
pub use req_sender::*;
use scenario::{ConcurrentUsersSameConnectionPool, ConcurrentUsersScenarioConfig, TestScenario};

/// Starts the measurement process.
pub async fn start(requests: Requests) -> Vec<Report> {
    let config = ConcurrentUsersScenarioConfig {
        nb_of_users: 10,
        nb_of_requests_per_user: 10,
    };
    let test_scenario = ConcurrentUsersSameConnectionPool::new(config);
    test_scenario.start_experiment(requests).await
}
