//! Tests for shutdown/restart cases.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use rand::Rng;
use sov_demo_rollup::MockDemoRollup;
use sov_mock_da::BlockProducingConfig;
use sov_modules_api::execution_mode::Native;
use sov_modules_api::OperatingMode;
use sov_stf_runner::processes::RollupProverConfig;
use sov_test_utils::test_rollup::{RollupBuilder, TestRollup};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{registry, Layer};

use crate::test_helpers::test_genesis_paths;

struct LogCollector {
    records: Arc<Mutex<Vec<(Level, String)>>>,
}

impl<S> Layer<S> for LogCollector
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let level = *event.metadata().level();

        if level <= Level::WARN {
            let mut message = String::new();
            let mut visitor = MessageVisitor(&mut message);
            event.record(&mut visitor);

            self.records.lock().unwrap().push((level, message));
        }
    }
}

struct MessageVisitor<'a>(&'a mut String);

impl<'a> tracing::field::Visit for MessageVisitor<'a> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0.push_str(&format!("{:?}", value));
        }
    }
}

/// Starts a TestNode, lets it run for some time and then shuts it down.
/// Repeats that several times.
/// Rollup and MockDa data are preserved between restarts.
async fn start_stop_empty(
    operation_mode: OperatingMode,
    finalization_blocks: u32,
    rollup_prover_config: RollupProverConfig,
) -> anyhow::Result<()> {
    let records = Arc::new(Mutex::new(Vec::new()));
    let collector = LogCollector {
        records: records.clone(),
    };
    let subscriber = registry().with(collector);
    subscriber.init();

    let mut rollup_storage_dir = tempfile::tempdir()?;
    let mock_da_dir = tempfile::tempdir()?;
    let restarts = 30;
    let mut rng = rand::thread_rng();

    let sleep_durations: Vec<std::time::Duration> = (0..restarts)
        .map(|_| std::time::Duration::from_millis(rng.gen_range(80..=300)))
        .collect();

    for sleep_duration in sleep_durations {
        let test_rollup = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            RollupBuilder::<MockDemoRollup<Native>>::start_memory_da_rollup_in_the_background_with_storage_dir(
                rollup_prover_config,
                &test_genesis_paths(operation_mode),
                rollup_storage_dir,
                BlockProducingConfig::Periodic,
                finalization_blocks,
                Some(mock_da_dir.path()),
            ),
        )
        .await??;

        // Let rollup run for some time
        tokio::time::sleep(sleep_duration).await;

        let TestRollup {
            storage_dir,
            shutdown_sender,
            rollup_task,
            da_service,
            ..
        } = test_rollup;

        let storable_mock_da = da_service.da_service();
        let block_producing_handle = storable_mock_da.take_block_producing_handle().unwrap();
        drop(da_service);
        tracing::info!("Triggering shutdown....");
        shutdown_sender.send(())?;
        tokio::time::timeout(std::time::Duration::from_secs(5), rollup_task).await???;
        rollup_storage_dir = storage_dir;
        block_producing_handle.await?;
    }

    let known = [
        // https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1878
        (
            Level::ERROR,
            "Invalid proof outcome, Invalid(ProverPenalized(\"Prover penalized\"))".to_string(),
        ),
    ];

    let mut recorded_errors_warnings =
        HashSet::<(Level, String)>::from_iter(records.lock().unwrap().clone().iter().cloned());
    recorded_errors_warnings.retain(|e| !known.contains(e));
    // We could've checked `.is_empty`, but in case of failure, we will see errors immediately.
    assert_eq!(HashSet::<(Level, String)>::new(), recorded_errors_warnings);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_start_stop_zk_instant_finality() -> anyhow::Result<()> {
    start_stop_empty(OperatingMode::Zk, 0, RollupProverConfig::Skip).await?;
    // if can_execute_zk_guest() {
    //     start_stop_empty(OperatingMode::Zk, 0, RollupProverConfig::Execute).await?;
    // }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_start_stop_zk_non_instant_finality() -> anyhow::Result<()> {
    start_stop_empty(OperatingMode::Zk, 3, RollupProverConfig::Skip).await?;
    // if can_execute_zk_guest() {
    //     start_stop_empty(OperatingMode::Zk, 3, RollupProverConfig::Execute).await?;
    // }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_start_stop_optimistic_instant_finality() -> anyhow::Result<()> {
    start_stop_empty(OperatingMode::Optimistic, 0, RollupProverConfig::Skip).await?;
    // if can_execute_zk_guest() {
    //     start_stop_empty(OperatingMode::Optimistic, 0, RollupProverConfig::Execute).await?;
    // }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_start_stop_optimistic_non_instant_finality() -> anyhow::Result<()> {
    start_stop_empty(OperatingMode::Optimistic, 3, RollupProverConfig::Skip).await?;
    // if can_execute_zk_guest() {
    //     start_stop_empty(OperatingMode::Optimistic, 3, RollupProverConfig::Execute).await?;
    // }
    Ok(())
}

// fn can_execute_zk_guest() -> bool {
//     let skip_guest_build = std::env::var("SKIP_GUEST_BUILD").unwrap_or_default();
//     matches!(skip_guest_build.to_lowercase().as_str(), "" | "0" | "false")
// }
