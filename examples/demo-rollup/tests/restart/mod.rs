//! Tests for shutdown/restart cases.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use ethers_providers::StreamExt;
use rand::Rng;
use sov_demo_rollup::MockDemoRollup;
use sov_mock_da::storable::layer::StorableMockDaLayer;
use sov_mock_da::BlockProducingConfig;
use sov_modules_api::execution_mode::Native;
use sov_modules_api::OperatingMode;
use sov_rollup_interface::da::BlockHeaderTrait;
use sov_stf_runner::processes::RollupProverConfig;
use sov_test_utils::test_rollup::{RollupBuilder, TestRollup};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{registry, Layer};

use crate::test_helpers::test_genesis_source;

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

    let rollup_storage_dir = Arc::new(tempfile::tempdir()?);
    let restarts = 50;
    let mut rng = rand::thread_rng();

    let sleep_durations: Vec<std::time::Duration> = (0..restarts)
        .map(|_| std::time::Duration::from_millis(rng.gen_range(80..=300)))
        .collect();

    for sleep_duration in sleep_durations {
        let test_rollup = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            RollupBuilder::<MockDemoRollup<Native>>::new(
                test_genesis_source(operation_mode),
                BlockProducingConfig::Periodic,
                finalization_blocks,
            )
            .set_config(|c| {
                c.storage = rollup_storage_dir.clone();
                c.rollup_prover_config = rollup_prover_config;
                c.aggregated_proof_block_jump = 10;
            })
            .start(),
        )
        .await??;

        // Let rollup run for some time
        tokio::time::sleep(sleep_duration).await;

        let TestRollup {
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
        block_producing_handle.await?;
    }

    let known = [
        // https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1878
        (
            Level::ERROR,
            "Invalid proof outcome, Invalid(ProverPenalized(\"Prover penalized\"))".to_string(),
        ),
        (
            Level::WARN,
            "The preferred sequencer is **experimental** and may not work as expected. Please report any issues you encounter.".to_string()
        )
    ];

    let mut recorded_errors_warnings =
        HashSet::<(Level, String)>::from_iter(records.lock().unwrap().clone().iter().cloned());
    recorded_errors_warnings.retain(|e| !known.contains(e));
    // We could've checked `.is_empty`, but in case of failure, we will see errors immediately.
    assert_eq!(HashSet::<(Level, String)>::new(), recorded_errors_warnings);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn flaky_test_start_stop_zk_instant_finality() -> anyhow::Result<()> {
    start_stop_empty(OperatingMode::Zk, 0, RollupProverConfig::Skip).await?;
    // if can_execute_zk_guest() {
    //     start_stop_empty(OperatingMode::Zk, 0, RollupProverConfig::Execute).await?;
    // }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn flaky_test_start_stop_zk_non_instant_finality() -> anyhow::Result<()> {
    start_stop_empty(OperatingMode::Zk, 3, RollupProverConfig::Skip).await?;
    // if can_execute_zk_guest() {
    //     start_stop_empty(OperatingMode::Zk, 3, RollupProverConfig::Execute).await?;
    // }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn flaky_test_start_stop_optimistic_instant_finality() -> anyhow::Result<()> {
    start_stop_empty(OperatingMode::Optimistic, 0, RollupProverConfig::Skip).await?;
    // if can_execute_zk_guest() {
    //     start_stop_empty(OperatingMode::Optimistic, 0, RollupProverConfig::Execute).await?;
    // }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn flaky_test_start_stop_optimistic_non_instant_finality() -> anyhow::Result<()> {
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

#[tokio::test(flavor = "multi_thread")]
async fn test_start_prover_manual() -> anyhow::Result<()> {
    let records = Arc::new(Mutex::new(Vec::new()));
    let collector = LogCollector {
        records: records.clone(),
    };
    let subscriber = registry().with(collector);
    subscriber.init();

    let rollup_storage_dir = Arc::new(tempfile::tempdir()?);
    let finalization_blocks = 0;

    let first_chunk = 6;
    let second_chunk = 4;
    let jump_size = first_chunk + second_chunk;

    let rollup_builder = RollupBuilder::<MockDemoRollup<Native>>::new(
        test_genesis_source(OperatingMode::Zk),
        BlockProducingConfig::OnAnySubmit,
        finalization_blocks,
    )
    .set_config(|c| {
        c.storage = rollup_storage_dir.clone();
        c.rollup_prover_config = RollupProverConfig::Skip;
        c.aggregated_proof_block_jump = jump_size;
    });

    let mock_da_dir = &rollup_storage_dir;

    {
        let mut storable_mock_da_layer =
            StorableMockDaLayer::new_in_path(mock_da_dir.path(), 0).await?;
        for _ in 0..first_chunk {
            storable_mock_da_layer.produce_block().await?;
        }
    }

    {
        let test_rollup = rollup_builder.clone().start().await?;
        let mut slot_subscription = test_rollup.client.client.subscribe_slots().await?;

        let TestRollup {
            shutdown_sender,
            rollup_task,
            ..
        } = test_rollup;

        let rollup_height = slot_subscription
            .next()
            .await
            .transpose()?
            .map(|slot| slot.number)
            .unwrap_or_default();

        if rollup_height < first_chunk as u64 {
            let till = first_chunk - rollup_height as usize;
            for _ in 0..till {
                let _ = slot_subscription.next().await.unwrap();
            }
        }
        drop(slot_subscription);

        shutdown_sender.send(())?;
        let _ = rollup_task.await?;
    }

    let _head_before_restart = {
        let mut storable_mock_da_layer =
            StorableMockDaLayer::new_in_path(mock_da_dir.path(), 0).await?;
        for _ in 0..=second_chunk {
            storable_mock_da_layer.produce_block().await?;
        }
        storable_mock_da_layer
            .get_head_block_header()
            .await?
            .height()
    };

    {
        let test_rollup = rollup_builder.start().await?;

        let mut slot_subscription = test_rollup.client.client.subscribe_slots().await?;

        let TestRollup {
            shutdown_sender,
            rollup_task,
            ..
        } = test_rollup;

        let rollup_height = slot_subscription
            .next()
            .await
            .transpose()?
            .map(|slot| slot.number)
            .unwrap_or_default();
        if rollup_height < second_chunk as u64 {
            let till = second_chunk - rollup_height as usize;
            for _ in 0..till {
                let _ = slot_subscription.next().await.unwrap();
            }
        }
        drop(slot_subscription);

        // FIXME(@theochap, `<https://github.com/Sovereign-Labs/sovereign-sdk-wip/issues/1907>`): this assertion is broken because of a race condition in the preferred sequencer.
        // // We give rollup 1 second to produce mock proof.
        // for _ in 0..10 {
        //     let head_after_restart = da_service.get_head_block_header().await?;
        //     if head_after_restart.height() > head_before_restart {
        //         break;
        //     }
        //     tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        // }

        // let head_after_restart = da_service.get_head_block_header().await?;
        // assert_eq!(
        //     head_after_restart.height(),
        //     head_before_restart + 1,
        //     "Prover hasn't posted proof"
        // );

        shutdown_sender.send(())?;
        let _ = rollup_task.await?;
    }

    let mut recorded_errors_warnings =
        HashSet::<(Level, String)>::from_iter(records.lock().unwrap().clone().iter().cloned());
    let known = [
        // Error because of ledger subscription
        (Level::WARN, "WebSocket error".to_string()),
        (Level::WARN, "The preferred sequencer is **experimental** and may not work as expected. Please report any issues you encounter.".to_string())
    ];
    recorded_errors_warnings.retain(|e| !known.contains(e));
    // We could've checked `.is_empty`, but in case of failure, we will see errors immediately.
    assert_eq!(HashSet::<(Level, String)>::new(), recorded_errors_warnings);

    Ok(())
}
