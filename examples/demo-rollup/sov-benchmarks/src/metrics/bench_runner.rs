//! A file that contains utilities to run benchmarks and gather metrics.

use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;
use std::time::Duration;

use anyhow::bail;
use demo_stf::runtime::{Runtime, RuntimeCall};
use sov_benchmarks::generator::BenchmarkData;
use sov_benchmarks::BenchRisc0Spec;
use sov_node_client::NodeClient;
use sov_test_utils::test_rollup::{RollupBuilder, TestRollup};
use sov_test_utils::RtAgnosticBlueprint;
use sov_transaction_generator::generators::basic::{
    BasicChangeLogEntry, BasicClientConfig, BasicTag,
};
use sov_transaction_generator::{
    assert_logs_against_state, GeneratedMessage, MessageOutcome, State,
};
use tokio::time::timeout;

use crate::helpers::{setup, BatchSender};

pub type S = BenchRisc0Spec;
pub type RT = Runtime<S>;
pub type BenchBlueprint = RtAgnosticBlueprint<S, RT>;
pub type BenchRollup = TestRollup<BenchBlueprint>;
pub type BenchRollupBuilder = RollupBuilder<BenchBlueprint>;
pub type BenchState = State<S, BasicTag>;
pub type BenchLogs = BasicChangeLogEntry<S>;
pub type BenchMessage = GeneratedMessage<S, RuntimeCall<S>, BenchLogs>;
pub type BenchOutcome = MessageOutcome<BasicChangeLogEntry<S>>;

/// Parses the next slot from a bench file.
pub fn parse_next_data(reader: &mut BufReader<File>) -> anyhow::Result<BenchmarkData<S>> {
    Ok(bincode::deserialize_from(reader)?)
}

/// Runs a bench file and gathers metrics.
pub async fn run_bench_file(bench_file: File, maybe_logs: Option<u8>) -> anyhow::Result<()> {
    // Starts by setting up the rollup for the benchmarks.
    let mut reader = BufReader::new(bench_file);

    let Ok(BenchmarkData::Genesis(genesis_config)) = parse_next_data(&mut reader) else {
        bail!("The bench file should start with an initialization slot. The bench file is invalid");
    };
    let rollup = setup(genesis_config).await?;
    let client = NodeClient::new(rollup.api_client.baseurl()).await?;
    let mut batch_sender = BatchSender::new(client).await;

    let Ok(BenchmarkData::Initialization(init_slot)) = parse_next_data(&mut reader) else {
        bail!("The bench file should start with an initialization slot. The bench file is invalid");
    };

    let mut log_accumulator = maybe_logs.map(|_| Vec::<BenchLogs>::new());

    let mut logs = batch_sender.produce_and_publish_batch(init_slot).await?;
    if let Some(acc) = log_accumulator.as_mut() {
        acc.append(&mut logs)
    }

    while let Ok(bench) = parse_next_data(&mut reader) {
        let slot = match bench {
            BenchmarkData::Execution {
                batches,
                slot_number,
            } => {
                println!("\nExecuting slot {}...", slot_number);
                batches
            }
            _ => {
                panic!("Expected an execution slot.")
            }
        };

        for (i, batch) in slot.into_iter().enumerate() {
            println!("Publishing batch number: {i}...");
            let mut logs = batch_sender.produce_and_publish_batch(batch).await?;
            if let Some(acc) = log_accumulator.as_mut() {
                acc.append(&mut logs)
            }
        }

        // We wait for the results to be in, so that we can be sure that the batches run in separate slots
        println!("Waiting for submission results...");
        timeout(Duration::from_secs(60), batch_sender.wait_for_results()).await??;
    }

    if let Some((assert_logs, log_accumulator)) = maybe_logs.zip(log_accumulator) {
        println!("\nAsserting logs...");
        assert_logs_against_state(
            log_accumulator,
            Arc::new(BasicClientConfig {
                url: rollup.api_client.baseurl().clone(),
                rollup_height: None,
            }),
            assert_logs,
        )
        .await?;
    }

    println!("\nShutting down rollup...");

    rollup.shutdown_sender.send(())?;
    let _x = rollup.rollup_task.await?;
    Ok(())
}
