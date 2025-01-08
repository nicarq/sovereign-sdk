//! Runs a benchmark of the zkvm metrics.

use std::fs::{self, File};
use std::path::Path;

use bench_runner::run_bench_file;
use clap::Parser;

pub mod bench_runner;
pub mod helpers;

const DEFAULT_BENCH_FILES: &str = "./src/bench_files/generated";
const DEFAULT_METRICS_OUTPUT: &str = "./src/metrics/generated";

#[derive(Parser, Debug)]
pub struct BenchMetricsCLI {
    /// Path to the bench files.
    #[clap(short, long, default_value_t = DEFAULT_BENCH_FILES.to_string())]
    path: String,
    /// Output directory for the metrics.
    #[clap(short, long, default_value_t = DEFAULT_METRICS_OUTPUT.to_string())]
    output: String,
    /// If set to `Some`, then asserts the logs against the state. The inner value is the maximal number of
    /// concurrent requests to the node. If set to `None`, then no state assertions are performed.
    #[arg(short, long)]
    logs: Option<u8>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse the path of the bench files.
    let cli = BenchMetricsCLI::parse();

    // Crawl through the directory and parse all the bench files.
    let dir = Path::new(&cli.path);
    let bench_files = fs::read_dir(dir).expect("Failed to read bench directory");
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

        println!("\nRunning bench file at path: {}.", entry.path().display());

        // Run the bench file.
        run_bench_file(bench_file, cli.logs).await?;
    }

    Ok(())
}
