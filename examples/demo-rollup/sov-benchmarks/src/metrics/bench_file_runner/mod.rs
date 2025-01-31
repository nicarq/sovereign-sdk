//! A file that contains utilities to run benchmarks and gather metrics.

use std::fs::File;
use std::io::BufReader;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use demo_stf::runtime::{Runtime, RuntimeCall};
use helpers::{setup_rollup, BatchReceiver, BatchSender};
use metrics::get_metrics;
use sov_benchmarks::generator::BenchmarkData;
use sov_benchmarks::BenchRisc0Spec;
use sov_metrics::{timestamp, METRICS_METADATA};
use sov_test_utils::test_rollup::{RollupBuilder, TestRollup};
use sov_test_utils::RtAgnosticBlueprint;
use sov_transaction_generator::generators::basic::{
    BasicChangeLogEntry, BasicClientConfig, BasicTag,
};
use sov_transaction_generator::{
    assert_logs_against_state, GeneratedMessage, MessageOutcome, State,
};
use tokio::sync::mpsc;

use crate::{BenchMetricsCLI, MetricsCLI, DEFAULT_TELEGRAF_ADDRESS};

pub type S = BenchRisc0Spec;
pub type RT = Runtime<S>;
pub type BenchBlueprint = RtAgnosticBlueprint<S, RT>;
pub type BenchRollup = TestRollup<BenchBlueprint>;
pub type BenchRollupBuilder = RollupBuilder<BenchBlueprint>;
pub type BenchState = State<S, BasicTag>;
pub type BenchLogs = BasicChangeLogEntry<S>;
pub type BenchMessage = GeneratedMessage<S, RuntimeCall<S>, BenchLogs>;
pub type BenchOutcome = MessageOutcome<BasicChangeLogEntry<S>>;

pub mod helpers;
pub mod metrics;

pub struct ParsedMetricsParameters {
    pub(crate) influx_address: SocketAddr,
    pub(crate) output_file: File,
    pub(crate) query_filter: String,
    pub(crate) influx_auth_token: String,
    pub(crate) influx_org_id: String,
    pub(crate) encoded: bool,
}

/// Parses the next slot from a bench file.
pub fn parse_next_data(reader: &mut BufReader<File>) -> anyhow::Result<BenchmarkData<S>> {
    Ok(bincode::deserialize_from(reader)?)
}

/// Runs a bench file and gathers metrics.
async fn runner(
    bench_name: String,
    bench_file: File,
    maybe_logs: Option<u8>,
    telegraf_address: SocketAddr,
    maybe_metrics_params: Option<ParsedMetricsParameters>,
) -> anyhow::Result<()> {
    // Starts by setting up the rollup for the benchmarks.
    let mut reader = BufReader::new(bench_file);

    let Ok(BenchmarkData::Genesis(genesis_config)) = parse_next_data(&mut reader) else {
        bail!("{bench_name}: The bench file should start with an initialization slot. The bench file is invalid");
    };
    let rollup = setup_rollup(genesis_config, telegraf_address).await?;

    let (batch_sender, batch_receiver) = mpsc::channel(100);
    let mut batch_sender = BatchSender::new(rollup.client.clone(), batch_sender).await;
    let batch_receiver = BatchReceiver::new(rollup.client.clone(), batch_receiver).await;

    let receiver_handle = batch_receiver.start_receiver(bench_name.clone());

    let Ok(BenchmarkData::Initialization(init_slot)) = parse_next_data(&mut reader) else {
        bail!("{bench_name}: The bench file should start with an initialization slot. The bench file is invalid");
    };

    let mut log_accumulator = maybe_logs.map(|_| Vec::<BenchLogs>::new());

    let mut logs = batch_sender.produce_and_publish_batch(init_slot).await?;
    if let Some(acc) = log_accumulator.as_mut() {
        acc.append(&mut logs)
    }

    let bench_start = timestamp();

    while let Ok(bench) = parse_next_data(&mut reader) {
        let slot_start = timestamp();
        let (slot_number, slot) = match bench {
            BenchmarkData::Execution {
                batches,
                slot_number,
            } => {
                println!("{bench_name}: Executing slot {}...", slot_number);
                (slot_number, batches)
            }
            _ => {
                panic!("{bench_name}: Expected an execution slot.")
            }
        };

        // FIXME: here we flatten the batches because the preferred sequencer can only send one batch per slot.
        // Ideally we should have a batch sender that can send multiple batches per slot.
        let batches = slot.into_iter().flatten().collect::<Vec<_>>();

        let num_txs = batches.len();

        let mut logs = batch_sender
            .produce_and_publish_batch(batches)
            .await
            .with_context(|| format!("{bench_name}: Failed to produce and publish batch"))?;
        if let Some(acc) = log_accumulator.as_mut() {
            acc.append(&mut logs)
        }
        let slot_end = timestamp();

        let exec_duration = Duration::from_nanos((slot_end - slot_start) as u64);
        let throughput = num_txs as f64 / exec_duration.as_secs_f64();

        println!(
            "{bench_name}: Slot {slot_number} execution took {} ms. Executed {} transactions. Throughtput {} txs/sec. Producing block...",
            exec_duration.as_millis(), num_txs, throughput.round()
        );
    }

    // We wait for all the results to be in.
    println!("{bench_name}: Waiting for submission results for the last slots...");

    // We drop the sender to ensure that the sender thread is closed which triggers completion of the receiver handle.
    drop(batch_sender);

    receiver_handle.await??;

    let bench_end = timestamp();

    // Query metrics and assert logs in separate threads. Both operations are independent of each other.
    // and may take a long time to complete.
    let mut joinset = tokio::task::JoinSet::new();

    if let Some(metrics_params) = maybe_metrics_params {
        joinset.spawn(async move {
            get_metrics(bench_start, bench_end, metrics_params)
                .await
                .expect("Failed to query metrics");
        });
    }

    // Assert logs (if necessary) and shut down the rollup afterwards.
    // Note that to assert the logs, the rollup still must be running (for the REST-api to be available).
    joinset.spawn(async move {
        if let Some((assert_logs, log_accumulator)) = maybe_logs.zip(log_accumulator) {
            println!("{bench_name}: Asserting logs...");
            assert_logs_against_state(
                log_accumulator,
                Arc::new(BasicClientConfig {
                    url: rollup.api_client.baseurl().clone(),
                    rollup_height: None,
                }),
                assert_logs,
            )
            .await
            .expect("Failed to assert logs");
        }

        println!("{bench_name}: Shutting down rollup...");

        rollup
            .shutdown_sender
            .send(())
            .expect("Failed to send shutdown signal");
        let _x = rollup
            .rollup_task
            .await
            .expect("Failed to join rollup task");
    });

    // Wait for all the tasks to finish and propagate any errors.
    joinset.join_all().await;

    Ok(())
}

pub async fn run_bench_file(input: BenchMetricsCLI) {
    // Parse the metrics CLI parameters.
    let (maybe_metrics_params, telegraf_address) = match input.metrics {
        Some(MetricsCLI::Metrics {
            telegraf,
            influx,
            influx_auth_token,
            influx_org_id,
            output,
            encoded,
            parameters,
        }) => {
            // Perform health checks for influxdb instance.
            match reqwest::Client::new()
                .get(format!("http://{}/health", influx))
                .send()
                .await
            {
                Ok(response) => {
                    if response.status() != 200 {
                        panic!("Unhealthy influxdb instance: {}. Please ensure that the instance is properly set up.", influx);
                    }
                }
                Err(_) => {
                    panic!("Invalid influxdb address: {}. Please ensure that the address is correct and that the service is running.", influx);
                }
            };

            let mut file_path = PathBuf::from(input.path.clone());

            // If the metrics are encoded, then we need to add the .gzip extension.
            if encoded {
                file_path.set_extension("csv.gz");
            } else {
                file_path.set_extension("csv");
            }

            let output_dir = PathBuf::from(output);

            (
                Some(ParsedMetricsParameters {
                    output_file: File::create(output_dir.join(file_path.file_name().unwrap()))
                        .unwrap(),
                    influx_address: influx,
                    influx_auth_token,
                    influx_org_id,
                    encoded,
                    query_filter: parameters.map(|p| p.format()).unwrap_or_default(),
                }),
                telegraf,
            )
        }
        _ => (None, DEFAULT_TELEGRAF_ADDRESS),
    };

    println!("Running bench file at path: {}.", input.path);

    let file_path = PathBuf::from(input.path.clone());
    let bench_file = File::open(file_path.clone())
        .unwrap_or_else(|_| panic!("Failed to open bench file at path {}. Make sure you provided an appropriate file name!", file_path.display()));
    let bench_name = file_path.file_stem().unwrap();
    let bench_name_str = bench_name.to_str().unwrap();

    // Setting the metrics metadata
    if METRICS_METADATA
        .write()
        .unwrap()
        .insert("bench_file".to_string(), bench_name_str.to_string())
        .is_some()
    {
        panic!("Impossible to insert metrics metadata")
    }

    // Run the bench file.
    runner(
        String::from(bench_name_str),
        bench_file,
        input.logs,
        // We always start the metrics sender for any rollup. If we don't expect metrics, we just pass the default address.
        telegraf_address,
        maybe_metrics_params,
    )
    .await
    .unwrap_or_else(|e| panic!("{bench_name_str}: Impossible to run bench file. Err {e})"));
}
