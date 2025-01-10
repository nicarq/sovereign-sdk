//! Runs a benchmark of the zkvm metrics.

use std::fs::{self, File};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};

use bench_runner::run_bench_file;
use clap::{Parser, Subcommand};
use sov_metrics::{MonitoringConfig, SovRollupMetrics};

pub mod bench_runner;
pub mod helpers;
pub mod metrics;

const DEFAULT_BENCH_FILES: &str = "./src/bench_files/generated";
const DEFAULT_METRICS_OUTPUT: &str = "./src/metrics/generated";
const DEFAULT_TELEGRAF_ADDRESS: SocketAddr = MonitoringConfig::standard().telegraf_address;
const DEFAULT_INFLUX_DB_ADDRESS: SocketAddr =
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 8086));

#[derive(Parser, Debug)]
pub struct BenchMetricsCLI {
    /// Path to the bench files.
    #[clap(short, long, default_value_t = DEFAULT_BENCH_FILES.to_string())]
    path: String,
    /// If set, then asserts the logs against the state. The inner value is the maximal number of
    /// concurrent requests to the node. If not specified, then no state assertions are performed.
    #[arg(short, long)]
    logs: Option<u8>,
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

pub struct ParsedMetricsParameters {
    pub(crate) influx_address: SocketAddr,
    pub(crate) output_file: File,
    pub(crate) query_filter: String,
    pub(crate) influx_auth_token: String,
    pub(crate) influx_org_id: String,
    pub(crate) encoded: bool,
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

    // Crawl through the directory and parse all the bench files.
    let dir = Path::new(&cli.path);
    let bench_files = fs::read_dir(dir).expect("Failed to read bench directory");

    // Parse the metrics CLI parameters.
    let (glob_maybe_metrics_params, telegraf_address) = match cli.metrics {
        Some(MetricsCLI::Metrics {
            telegraf,
            influx,
            influx_auth_token,
            influx_org_id,
            output,
            encoded,
            parameters,
        }) => {
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

            (
                Some((
                    PathBuf::from(&output),
                    influx,
                    influx_auth_token,
                    influx_org_id,
                    encoded,
                    parameters.map(|p| p.format()).unwrap_or_default(),
                )),
                telegraf,
            )
        }
        _ => (None, DEFAULT_TELEGRAF_ADDRESS),
    };

    for bench_file in bench_files {
        let entry = bench_file.expect("Failed to read directory entry");

        // Only parse files.
        if !entry
            .file_type()
            .unwrap_or_else(|_| {
                panic!(
                    "Failed to parse file type for bench file at path {}",
                    entry.path().display()
                )
            })
            .is_file()
        {
            continue;
        }

        let bench_file = File::open(entry.path()).unwrap_or_else(|_| {
            panic!(
                "Failed to open bench file at path {}",
                entry.path().display()
            )
        });

        // If there are metrics CLI parameters, then we need to open a file for the metrics output for this benchmark.
        let bench_maybe_metrics_params = glob_maybe_metrics_params.clone().map(
            |(dir, influx, influx_auth_token, influx_org_id, encoded, filter)| {
                let mut file_path = entry.path();

                // If the metrics are encoded, then we need to add the .gzip extension.
                if encoded {
                    file_path.set_extension("csv.gz");
                } else {
                    file_path.set_extension("csv");
                }

                ParsedMetricsParameters {
                    output_file: File::create(dir.join(file_path.file_name().unwrap())).unwrap(),
                    influx_address: influx,
                    influx_auth_token,
                    influx_org_id,
                    encoded,
                    query_filter: filter,
                }
            },
        );

        println!("\nRunning bench file at path: {}.", entry.path().display());

        // Run the bench file.
        run_bench_file(
            bench_file,
            cli.logs,
            // We always start the metrics sender for any rollup. If we don't expect metrics, we just pass the default address.
            telegraf_address,
            bench_maybe_metrics_params,
        )
        .await?;
    }

    Ok(())
}
