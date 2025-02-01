//! Runs a benchmark of the zkvm metrics.

use std::env;
use std::fs::{self};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;

use anyhow::Context;
use bench_file_runner::run_bench_file;
use clap::{Parser, Subcommand};
use sov_metrics::{MonitoringConfig, SovRollupMetrics};
use sov_test_utils::initialize_logging;
use tracing::info_span;

mod bench_file_runner;

const DEFAULT_BENCH_FILES: &str = "./src/bench_files/generated";
const DEFAULT_METRICS_OUTPUT: &str = "./src/metrics/generated";
const DEFAULT_TELEGRAF_ADDRESS: SocketAddr = MonitoringConfig::standard().telegraf_address;
const DEFAULT_INFLUX_DB_ADDRESS: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 8086));
const DEFAULT_NUM_THREADS: u8 = 10;

#[derive(Parser, Clone, Debug)]
pub struct BenchMetricsCLI {
    /// Path to the bench files. It can be either a folder name or a specific file.
    /// If the path points to a folder, then all the bench files inside the folder will be run
    /// in a separate process.
    #[clap(short, long, default_value_t = DEFAULT_BENCH_FILES.to_string())]
    pub path: String,
    /// If set, then asserts the logs against the state. The inner value is the maximal number of
    /// concurrent requests to the node. If not specified, then no state assertions are performed.
    #[arg(short, long)]
    logs: Option<u8>,
    #[arg(short, long, default_value_t = DEFAULT_NUM_THREADS)]
    /// Maximum number of concurrent threads to run the benchmarks.
    threads: u8,
    /// Specifies how to store and query the metrics. If not specified, no metrics are stored.
    #[command(subcommand)]
    metrics: Option<MetricsCLI>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum MetricsCLI {
    /// Track metrics using telegraf.
    Metrics {
        /// Address of the telegraf service. Make sure that the service is up and running before running this executable.
        #[arg(short, long, default_value_t = DEFAULT_TELEGRAF_ADDRESS)]
        telegraf: SocketAddr,
        /// Address of the influxdb service. Make sure that the service is up and running before running this executable.
        #[arg(short, long, default_value_t = DEFAULT_INFLUX_DB_ADDRESS)]
        influx: SocketAddr,
        /// Influx token to authenticate with the influxdb service.
        #[arg(long)]
        influx_auth_token: String,
        /// Influx org id to authenticate with the influxdb service.
        #[arg(long)]
        influx_org_id: String,
        /// Output directory for the metrics.
        #[arg(short, long, default_value_t = DEFAULT_METRICS_OUTPUT.to_string())]
        output: String,
        /// Whether to encode the metrics in gzip format. By default, the metrics are not encoded.
        #[arg(short, long, default_value_t = false)]
        encoded: bool,
        /// Additional parameters to apply to the metrics. If not specified, then no filtering is applied.
        #[command(subcommand)]
        parameters: Option<MetricsQueryParameters>,
    },
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum MetricsQueryParameters {
    /// Only keep the following measurements.
    Measurements {
        sov_rollup_metrics: Vec<SovRollupMetrics>,
    },
    /// Runs a custom query. Must be a valid flux query parameter.
    /// Examples of query parameters:
    /// ```
    /// range(start: -1h)
    /// filter(fn: (r) => r._measurement == "example-measurement" and r._field == "example-field")
    /// filter(fn: (r) => r._measurement == "example-measurement_b" and r._field == "example-field_b")
    /// ```
    Custom { query_filters: Vec<String> },
}

impl MetricsQueryParameters {
    /// Formats the query filters into a valid flux query.
    fn format(self) -> String {
        let query_vec = match self {
            Self::Measurements { sov_rollup_metrics } => sov_rollup_metrics
                .iter()
                .map(|m| format!("r._measurement == \"{}\"", m.measurement_name()))
                .collect::<Vec<_>>(),
            Self::Custom { query_filters } => query_filters,
        };

        format!("|> filter(fn: (r) => {})", query_vec.join(" or "))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse the path of the bench files.
    let cli = BenchMetricsCLI::parse();

    // We only set the logging level if the env var is not set.
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "warn,error,bench_runner=info");
    }

    initialize_logging();

    info_span!("bench_runner");

    let path = PathBuf::from(&cli.path);

    if !path.exists() {
        panic!(
            "Bench file path {path:?} does not exist. Please make sure you provided a valid path."
        );
    }

    // If the path points to a file, then we simply run the bench file.
    if path.is_file() {
        run_bench_file(cli).await;
        return Ok(());
    }

    // If the path points to a directory, then we run all the bench files inside the directory.
    let bench_files = fs::read_dir(path).unwrap_or_else(|err| panic!("Failed to read bench directory: {err}. Please make sure you provide a valid directory or file path!"));

    // Spawn a child process for each bench file. Await the processes concurrently
    let mut processes = Vec::new();

    for bench_file in bench_files {
        // We remove both the path and its argument if it exist. They are replaced when calling the process.
        // Note that `position` consumes the underlying iterator, hence the need to call [`args_os`] again afterwards.
        let maybe_pos = std::env::args_os()
            .position(|item| item.to_str().unwrap() == "-p" || item.to_str().unwrap() == "--path");

        let args = std::env::args_os();

        let args: Vec<_> = if let Some(pos) = maybe_pos {
            args.enumerate()
                .filter_map(|(i, item)| {
                    if i != pos && i != pos + 1 {
                        Some(item)
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            args.collect()
        };

        let file_path = bench_file.unwrap().path();

        // The first argument is always the process name.
        let handle = tokio::process::Command::new(args[0].clone())
            .args(vec![
                "-p",
                &file_path.into_os_string().into_string().unwrap(),
            ])
            .args(args.into_iter().skip(1))
            .kill_on_drop(true)
            .spawn()
            .with_context(|| "Impossible to create child process")?;

        processes.push(handle);
    }

    for mut process in processes {
        process.wait().await?;
    }

    Ok(())
}
