//! Tests for shutdown/restart cases.

use rand::Rng;
use sov_mock_da::BlockProducingConfig;
use sov_modules_api::OperatingMode;
use sov_stf_runner::processes::RollupProverConfig;

use crate::test_helpers::TestRollup;

/// Starts a TestNode, lets it run for some time and then shuts it down.
/// Repeats that several times.
/// Rollup and MockDa data are preserved between restarts.
async fn start_stop_empty(
    operation_mode: OperatingMode,
    finalization_blocks: u32,
    rollup_prover_config: RollupProverConfig,
) -> anyhow::Result<()> {
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
            TestRollup::create_test_rollup(
                rollup_prover_config,
                operation_mode,
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
            ..
        } = test_rollup;

        tracing::info!("Triggering shutdown....");
        shutdown_sender.send(())?;
        tokio::time::timeout(std::time::Duration::from_secs(5), rollup_task).await???;
        rollup_storage_dir = storage_dir;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_start_stop_zk_instant_finality() -> anyhow::Result<()> {
    start_stop_empty(OperatingMode::Zk, 0, RollupProverConfig::Skip).await
    // start_stop_empty(OperatingMode::Zk, 0, RollupProverConfig::Execute).await
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn test_start_stop_zk_non_instant_finality() -> anyhow::Result<()> {
    start_stop_empty(OperatingMode::Zk, 3, RollupProverConfig::Skip).await
    // start_stop_empty(OperatingMode::Zk, 3, RollupProverConfig::Execute).await
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn test_start_stop_optimistic_instant_finality() -> anyhow::Result<()> {
    start_stop_empty(OperatingMode::Optimistic, 0, RollupProverConfig::Skip).await
    // start_stop_empty(OperatingMode::Optimistic, 0, RollupProverConfig::Execute).await
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn test_start_stop_optimistic_non_instant_finality() -> anyhow::Result<()> {
    start_stop_empty(OperatingMode::Optimistic, 3, RollupProverConfig::Skip).await
}
